#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  copyFile,
  mkdir,
  open,
  readFile,
  realpath,
  stat,
  writeFile,
} from "node:fs/promises";
import { createReadStream } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const MAX_SCREENSHOT_BYTES = 50 * 1024 * 1024;
const MIN_SCREENSHOT_LONG_EDGE = 1366;
const MIN_SCREENSHOT_SHORT_EDGE = 768;
const MAX_DESKTOP_SCREENSHOTS = 10;
const STORE_PACKAGE_EXTENSIONS = new Set([
  ".msix",
  ".msixbundle",
  ".msixupload",
]);

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function formatValidationError(subject, errors) {
  return new Error(`${subject}:\n- ${errors.join("\n- ")}`);
}

function validateAssetPath(candidate) {
  if (typeof candidate !== "string" || candidate.trim() === "") {
    return "must be a non-empty relative PNG path";
  }
  if (
    candidate.includes("\\") ||
    candidate.startsWith("/") ||
    /^[a-zA-Z]:/.test(candidate) ||
    path.posix.normalize(candidate) !== candidate ||
    candidate.split("/").some((part) => part === "" || part === "." || part === "..")
  ) {
    return "must stay below store/listing/assets and use normalized '/' separators";
  }
  if (path.posix.extname(candidate).toLowerCase() !== ".png") {
    return "must point to a .png file";
  }
  return null;
}

/** Validate the checked-in screenshot manifest without touching the filesystem. */
export function validateStoreListingManifest(manifest) {
  const errors = [];
  if (!isRecord(manifest)) {
    return ["manifest must be a JSON object"];
  }

  const allowedKeys = new Set(["$schema", "productId", "locales"]);
  for (const key of Object.keys(manifest)) {
    if (!allowedKeys.has(key)) {
      errors.push(`unknown top-level field ${JSON.stringify(key)}`);
    }
  }

  if (
    typeof manifest.productId !== "string" ||
    !/^[A-Z0-9]{10,20}$/.test(manifest.productId)
  ) {
    errors.push("productId must be a 10-20 character uppercase Store product id");
  }

  if (!isRecord(manifest.locales)) {
    errors.push("locales must be an object");
    return errors;
  }

  for (const [locale, files] of Object.entries(manifest.locales)) {
    if (!/^[a-z]{2,3}(?:-[a-z0-9]{2,8})+$/.test(locale)) {
      errors.push(
        `${JSON.stringify(locale)} must be a lower-case BCP 47 locale such as en-us`,
      );
    }
    if (!Array.isArray(files)) {
      errors.push(`${locale} must be an ordered array of PNG paths`);
      continue;
    }
    if (files.length === 0) {
      errors.push(`${locale} cannot be empty; omit the locale to preserve its screenshots`);
    }
    if (files.length > MAX_DESKTOP_SCREENSHOTS) {
      errors.push(`${locale} has ${files.length} screenshots; Store allows at most 10`);
    }

    const seen = new Set();
    for (const [index, file] of files.entries()) {
      const pathError = validateAssetPath(file);
      if (pathError) {
        errors.push(`${locale}[${index}] ${pathError}`);
        continue;
      }
      const key = file.toLowerCase();
      if (seen.has(key)) {
        errors.push(`${locale} contains duplicate screenshot ${JSON.stringify(file)}`);
      }
      seen.add(key);
    }
  }

  return errors;
}

/** Read dimensions from the PNG signature and mandatory IHDR chunk. */
export function parsePngDimensions(buffer) {
  if (!Buffer.isBuffer(buffer) || buffer.length < 24) {
    throw new Error("file is too short to contain a PNG IHDR chunk");
  }
  if (!buffer.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) {
    throw new Error("PNG signature is invalid");
  }
  if (
    buffer.readUInt32BE(8) !== 13 ||
    buffer.subarray(12, 16).toString("ascii") !== "IHDR"
  ) {
    throw new Error("PNG does not begin with the mandatory IHDR chunk");
  }
  const width = buffer.readUInt32BE(16);
  const height = buffer.readUInt32BE(20);
  if (width === 0 || height === 0) {
    throw new Error("PNG dimensions must be non-zero");
  }
  return { width, height };
}

export function validateScreenshotMetadata({ size, width, height }) {
  const errors = [];
  if (!Number.isInteger(size) || size <= 0) {
    errors.push("file must not be empty");
  } else if (size > MAX_SCREENSHOT_BYTES) {
    errors.push("file exceeds the Store 50 MB limit");
  }
  const longEdge = Math.max(width, height);
  const shortEdge = Math.min(width, height);
  if (
    longEdge < MIN_SCREENSHOT_LONG_EDGE ||
    shortEdge < MIN_SCREENSHOT_SHORT_EDGE
  ) {
    errors.push(
      `dimensions ${width}x${height} are below the desktop minimum ` +
        "(1366x768 landscape or 768x1366 portrait)",
    );
  }
  return errors;
}

async function hashFile(filePath) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(filePath)) {
    hash.update(chunk);
  }
  return hash.digest("hex");
}

async function inspectPng(filePath) {
  const fileStat = await stat(filePath);
  if (!fileStat.isFile()) {
    throw new Error("path is not a regular file");
  }
  if (fileStat.size > MAX_SCREENSHOT_BYTES) {
    throw new Error("file exceeds the Store 50 MB limit");
  }

  const header = Buffer.alloc(24);
  const handle = await open(filePath, "r");
  try {
    const { bytesRead } = await handle.read(header, 0, header.length, 0);
    if (bytesRead < header.length) {
      throw new Error("file is too short to contain a PNG IHDR chunk");
    }
  } finally {
    await handle.close();
  }

  const { width, height } = parsePngDimensions(header);
  const metadataErrors = validateScreenshotMetadata({
    size: fileStat.size,
    width,
    height,
  });
  if (metadataErrors.length > 0) {
    throw new Error(metadataErrors.join("; "));
  }
  return {
    size: fileStat.size,
    width,
    height,
    digest: await hashFile(filePath),
  };
}

function isWithin(parent, candidate) {
  const relative = path.relative(parent, candidate);
  return relative !== "" && !relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative);
}

function uploadName(locale, relativePath, digest) {
  const rawStem = path.posix.basename(relativePath, path.posix.extname(relativePath));
  const stem =
    rawStem
      .normalize("NFKD")
      .replace(/[^a-zA-Z0-9._-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 64) || "screenshot";
  return `${locale}/Screenshot_${stem}-${digest.slice(0, 12)}.png`;
}

/** Load and fully validate all files referenced by a screenshot manifest. */
export async function loadStoreListingPlan(manifestPath) {
  const resolvedManifest = path.resolve(manifestPath);
  let manifest;
  try {
    manifest = JSON.parse(await readFile(resolvedManifest, "utf8"));
  } catch (error) {
    throw new Error(`could not read ${resolvedManifest}: ${error.message}`);
  }

  const shapeErrors = validateStoreListingManifest(manifest);
  if (shapeErrors.length > 0) {
    throw formatValidationError("invalid Store screenshot manifest", shapeErrors);
  }

  const assetRoot = path.resolve(path.dirname(resolvedManifest), "assets");
  const hasAssets = Object.values(manifest.locales).some((files) => files.length > 0);
  let realAssetRoot = null;
  if (hasAssets) {
    try {
      realAssetRoot = await realpath(assetRoot);
    } catch {
      throw new Error(`Store screenshot asset directory does not exist: ${assetRoot}`);
    }
  }

  const locales = {};
  const warnings = [];
  const archivePaths = new Set();
  const fileErrors = [];
  for (const [locale, files] of Object.entries(manifest.locales)) {
    if (files.length < 4) {
      warnings.push(
        `${locale} has ${files.length} screenshot(s); Microsoft recommends at least four`,
      );
    }
    locales[locale] = [];
    for (const relativePath of files) {
      try {
        const candidate = path.resolve(assetRoot, ...relativePath.split("/"));
        const sourcePath = await realpath(candidate);
        if (!isWithin(realAssetRoot, sourcePath)) {
          throw new Error("resolved path escapes store/listing/assets");
        }
        const metadata = await inspectPng(sourcePath);
        const archivePath = uploadName(locale, relativePath, metadata.digest);
        const archiveKey = archivePath.toLowerCase();
        if (archivePaths.has(archiveKey)) {
          throw new Error(`collides with another uploaded filename: ${archivePath}`);
        }
        archivePaths.add(archiveKey);
        locales[locale].push({
          relativePath,
          sourcePath,
          archivePath,
          ...metadata,
        });
      } catch (error) {
        fileErrors.push(`${locale}/${relativePath}: ${error.message}`);
      }
    }
  }
  if (fileErrors.length > 0) {
    throw formatValidationError("invalid Store screenshot assets", fileErrors);
  }

  return {
    manifestPath: resolvedManifest,
    productId: manifest.productId,
    locales,
    warnings,
  };
}

async function assertStorePackage(packagePath) {
  const resolved = path.resolve(packagePath);
  const extension = path.extname(resolved).toLowerCase();
  if (!STORE_PACKAGE_EXTENSIONS.has(extension)) {
    throw new Error(
      `Store package must be .msix, .msixbundle, or .msixupload: ${resolved}`,
    );
  }
  const packageStat = await stat(resolved);
  if (!packageStat.isFile() || packageStat.size === 0) {
    throw new Error(`Store package must be a non-empty file: ${resolved}`);
  }
  return { path: resolved, name: path.basename(resolved), extension };
}

function findListing(listings, locale) {
  const key = Object.keys(listings).find(
    (candidate) => candidate.toLowerCase() === locale,
  );
  return key ? { key, listing: listings[key] } : null;
}

/** Patch a fresh API-created submission and describe the files its ZIP must contain. */
export function patchStoreSubmission({ submission, packageInfo, listingPlan }) {
  if (!isRecord(submission)) {
    throw new Error("Store submission must be a JSON object");
  }
  if (!Array.isArray(submission.applicationPackages)) {
    throw new Error("Store submission has no applicationPackages array");
  }

  const patched = structuredClone(submission);
  const packageMatches = patched.applicationPackages.filter(
    (candidate) =>
      typeof candidate?.fileName === "string" &&
      path.extname(candidate.fileName).toLowerCase() === packageInfo.extension,
  );
  for (const existing of packageMatches) {
    if (
      existing.fileStatus === "PendingUpload" ||
      existing.fileStatus === "PendingDelete"
    ) {
      throw new Error(
        `submission package ${existing.fileName} is already ${existing.fileStatus}; ` +
          "only a clean inherited submission may be automated",
      );
    }
    existing.fileStatus = "PendingDelete";
  }
  patched.applicationPackages.push({
    fileName: packageInfo.name,
    fileStatus: "PendingUpload",
  });

  const localeEntries = Object.entries(listingPlan.locales);
  if (localeEntries.length > 0 && !isRecord(patched.listings)) {
    throw new Error("Store submission has no listings object");
  }

  const archiveEntries = [
    { sourcePath: packageInfo.path, archivePath: packageInfo.name },
  ];
  for (const [locale, screenshots] of localeEntries) {
    const match = findListing(patched.listings, locale);
    if (!match || !isRecord(match.listing?.baseListing)) {
      throw new Error(
        `Store listing ${locale} does not exist; create its localized text before syncing screenshots`,
      );
    }
    const images = match.listing.baseListing.images;
    if (images != null && !Array.isArray(images)) {
      throw new Error(`Store listing ${locale} has an invalid images collection`);
    }
    const nextImages = structuredClone(images ?? []);
    for (const image of nextImages) {
      if (String(image?.imageType).toLowerCase() !== "screenshot") {
        continue;
      }
      if (image.fileStatus === "PendingUpload") {
        throw new Error(
          `Store listing ${locale} already has a pending screenshot upload; ` +
            "only a clean inherited submission may be automated",
        );
      }
      image.fileStatus = "PendingDelete";
    }
    for (const screenshot of screenshots) {
      nextImages.push({
        fileName: screenshot.archivePath,
        fileStatus: "PendingUpload",
        imageType: "Screenshot",
      });
      archiveEntries.push({
        sourcePath: screenshot.sourcePath,
        archivePath: screenshot.archivePath,
      });
    }
    match.listing.baseListing.images = nextImages;
  }

  return {
    submission: patched,
    archiveEntries,
    replacedLocales: localeEntries.map(([locale]) => locale),
    screenshotCount: localeEntries.reduce(
      (total, [, screenshots]) => total + screenshots.length,
      0,
    ),
  };
}

async function readSubmission(submissionPath) {
  const resolved = path.resolve(submissionPath);
  let submission;
  try {
    submission = JSON.parse(await readFile(resolved, "utf8"));
  } catch (error) {
    throw new Error(`could not read Store submission fixture ${resolved}: ${error.message}`);
  }
  return submission;
}

async function buildPlan(manifestPath, submissionPath, packagePath) {
  const [listingPlan, submission, packageInfo] = await Promise.all([
    loadStoreListingPlan(manifestPath),
    readSubmission(submissionPath),
    assertStorePackage(packagePath),
  ]);
  const patched = patchStoreSubmission({ submission, packageInfo, listingPlan });
  return { listingPlan, packageInfo, ...patched };
}

export async function preflightStoreSubmission(
  manifestPath,
  submissionPath,
  packagePath,
) {
  const plan = await buildPlan(manifestPath, submissionPath, packagePath);
  return {
    productId: plan.listingPlan.productId,
    packageName: plan.packageInfo.name,
    replacedLocales: plan.replacedLocales,
    screenshotCount: plan.screenshotCount,
    warnings: plan.listingPlan.warnings,
  };
}

export async function stageStoreSubmission(
  manifestPath,
  submissionPath,
  packagePath,
  outputDirectory,
) {
  const plan = await buildPlan(manifestPath, submissionPath, packagePath);
  const outputRoot = path.resolve(outputDirectory);
  try {
    await stat(outputRoot);
    throw new Error(`refusing to reuse Store staging directory: ${outputRoot}`);
  } catch (error) {
    if (error.code !== "ENOENT") {
      throw error;
    }
  }

  const stagingDirectory = path.join(outputRoot, "upload");
  await mkdir(stagingDirectory, { recursive: true });
  for (const entry of plan.archiveEntries) {
    const destination = path.join(
      stagingDirectory,
      ...entry.archivePath.split("/"),
    );
    await mkdir(path.dirname(destination), { recursive: true });
    await copyFile(entry.sourcePath, destination);
  }

  const metadataPath = path.join(outputRoot, "submission.json");
  await writeFile(metadataPath, `${JSON.stringify(plan.submission, null, 2)}\n`, "utf8");
  return {
    productId: plan.listingPlan.productId,
    metadataPath,
    stagingDirectory,
    packageName: plan.packageInfo.name,
    replacedLocales: plan.replacedLocales,
    screenshotCount: plan.screenshotCount,
    warnings: plan.listingPlan.warnings,
  };
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "validate" && args.length === 1) {
    const plan = await loadStoreListingPlan(args[0]);
    for (const warning of plan.warnings) console.log(`warning: ${warning}`);
    const count = Object.values(plan.locales).reduce(
      (total, screenshots) => total + screenshots.length,
      0,
    );
    if (count === 0) {
      console.log("Store screenshot manifest is valid; existing listing screenshots will be preserved.");
    } else {
      console.log(
        `Store screenshot manifest is valid: ${count} screenshot(s) across ` +
          `${Object.keys(plan.locales).length} locale(s).`,
      );
    }
    return;
  }
  if (command === "preflight" && args.length === 3) {
    console.log(JSON.stringify(await preflightStoreSubmission(...args)));
    return;
  }
  if (command === "prepare" && args.length === 4) {
    console.log(JSON.stringify(await stageStoreSubmission(...args)));
    return;
  }
  throw new Error(
    "usage:\n" +
      "  node tools/store-listing.mjs validate <screenshots.json>\n" +
      "  node tools/store-listing.mjs preflight <screenshots.json> <submission.json> <package>\n" +
      "  node tools/store-listing.mjs prepare <screenshots.json> <submission.json> <package> <output-dir>",
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
