import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
  loadStoreListingPlan,
  parsePngDimensions,
  patchStoreSubmission,
  stageStoreSubmission,
  validateScreenshotMetadata,
  validateStoreListingManifest,
} from "./store-listing.mjs";

const temporaryDirectories = [];

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) =>
      rm(directory, { recursive: true, force: true }),
    ),
  );
});

async function temporaryDirectory() {
  const directory = await mkdtemp(path.join(tmpdir(), "gitwink-listing-test-"));
  temporaryDirectories.push(directory);
  return directory;
}

function pngHeader(width, height) {
  const buffer = Buffer.alloc(24);
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(buffer, 0);
  buffer.writeUInt32BE(13, 8);
  buffer.write("IHDR", 12, "ascii");
  buffer.writeUInt32BE(width, 16);
  buffer.writeUInt32BE(height, 20);
  return buffer;
}

async function listingFixture({ locales = { "en-us": ["en-us/01-main.png"] } } = {}) {
  const root = await temporaryDirectory();
  const listingDirectory = path.join(root, "store", "listing");
  const assetsDirectory = path.join(listingDirectory, "assets");
  await mkdir(assetsDirectory, { recursive: true });
  const manifest = {
    productId: "9P0S21GJD53F",
    locales,
  };
  const manifestPath = path.join(listingDirectory, "screenshots.json");
  await writeFile(manifestPath, JSON.stringify(manifest), "utf8");
  for (const files of Object.values(locales)) {
    for (const file of files) {
      const destination = path.join(assetsDirectory, ...file.split("/"));
      await mkdir(path.dirname(destination), { recursive: true });
      await writeFile(destination, pngHeader(1366, 768));
    }
  }
  return { root, listingDirectory, manifestPath };
}

function submissionFixture() {
  return {
    id: "draft-1",
    fileUploadUrl: "https://example.invalid/upload?sig=secret",
    applicationPackages: [
      {
        id: "old-package",
        fileName: "gitwink_0.11.0.msixbundle",
        fileStatus: "Uploaded",
      },
    ],
    listings: {
      "en-us": {
        baseListing: {
          title: "gitwink",
          images: [
            {
              id: "old-screenshot",
              fileName: "old.png",
              fileStatus: "Uploaded",
              imageType: "Screenshot",
            },
            {
              id: "store-logo",
              fileName: "logo.png",
              fileStatus: "Uploaded",
              imageType: "StoreLogoSquare",
            },
          ],
        },
      },
      "ko-kr": {
        baseListing: {
          title: "gitwink",
          images: [
            {
              id: "ko-screenshot",
              fileName: "ko.png",
              fileStatus: "Uploaded",
              imageType: "Screenshot",
            },
          ],
        },
      },
    },
  };
}

describe("Store screenshot manifest", () => {
  it("rejects typo fields, unsafe paths, duplicate files, and empty locale sets", () => {
    const errors = validateStoreListingManifest({
      productId: "9P0S21GJD53F",
      typo: true,
      locales: {
        "EN-us": [],
        "ko-kr": ["../outside.png", "same.png", "SAME.PNG"],
      },
    });
    expect(errors).toEqual(
      expect.arrayContaining([
        'unknown top-level field "typo"',
        '"EN-us" must be a lower-case BCP 47 locale such as en-us',
        "EN-us cannot be empty; omit the locale to preserve its screenshots",
        "ko-kr[0] must stay below store/listing/assets and use normalized '/' separators",
        'ko-kr contains duplicate screenshot "SAME.PNG"',
      ]),
    );
  });

  it("accepts landscape and portrait desktop minimums", () => {
    expect(parsePngDimensions(pngHeader(1366, 768))).toEqual({
      width: 1366,
      height: 768,
    });
    expect(
      validateScreenshotMetadata({ size: 24, width: 768, height: 1366 }),
    ).toEqual([]);
    expect(
      validateScreenshotMetadata({ size: 24, width: 1200, height: 700 }),
    ).toContain(
      "dimensions 1200x700 are below the desktop minimum (1366x768 landscape or 768x1366 portrait)",
    );
  });

  it("validates files and creates content-addressed upload names", async () => {
    const { manifestPath } = await listingFixture();
    const plan = await loadStoreListingPlan(manifestPath);
    expect(plan.locales["en-us"]).toHaveLength(1);
    expect(plan.locales["en-us"][0]).toMatchObject({
      relativePath: "en-us/01-main.png",
      width: 1366,
      height: 768,
      size: 24,
    });
    expect(plan.locales["en-us"][0].archivePath).toMatch(
      /^en-us\/Screenshot_01-main-[a-f0-9]{12}\.png$/,
    );
  });
});

describe("Store submission patching", () => {
  it("replaces only listed desktop screenshots and preserves other listing assets", async () => {
    const { manifestPath } = await listingFixture();
    const listingPlan = await loadStoreListingPlan(manifestPath);
    const result = patchStoreSubmission({
      submission: submissionFixture(),
      packageInfo: {
        path: "C:/build/gitwink_0.12.0.msixbundle",
        name: "gitwink_0.12.0.msixbundle",
        extension: ".msixbundle",
      },
      listingPlan,
    });

    expect(result.submission.applicationPackages).toEqual([
      expect.objectContaining({
        id: "old-package",
        fileStatus: "PendingDelete",
      }),
      {
        fileName: "gitwink_0.12.0.msixbundle",
        fileStatus: "PendingUpload",
      },
    ]);
    const englishImages =
      result.submission.listings["en-us"].baseListing.images;
    expect(englishImages).toEqual([
      expect.objectContaining({
        id: "old-screenshot",
        fileStatus: "PendingDelete",
      }),
      expect.objectContaining({
        id: "store-logo",
        fileStatus: "Uploaded",
      }),
      expect.objectContaining({
        imageType: "Screenshot",
        fileStatus: "PendingUpload",
      }),
    ]);
    expect(
      result.submission.listings["ko-kr"].baseListing.images[0].fileStatus,
    ).toBe("Uploaded");
    expect(result.replacedLocales).toEqual(["en-us"]);
    expect(result.screenshotCount).toBe(1);
  });

  it("preserves every screenshot when no locale is listed", async () => {
    const { manifestPath } = await listingFixture({ locales: {} });
    const listingPlan = await loadStoreListingPlan(manifestPath);
    const result = patchStoreSubmission({
      submission: submissionFixture(),
      packageInfo: {
        path: "C:/build/gitwink.msixbundle",
        name: "gitwink.msixbundle",
        extension: ".msixbundle",
      },
      listingPlan,
    });
    expect(
      result.submission.listings["en-us"].baseListing.images[0].fileStatus,
    ).toBe("Uploaded");
    expect(result.screenshotCount).toBe(0);
  });

  it("refuses to invent localized listing text", async () => {
    const { manifestPath } = await listingFixture({
      locales: { "ja-jp": ["ja-jp/main.png"] },
    });
    const listingPlan = await loadStoreListingPlan(manifestPath);
    expect(() =>
      patchStoreSubmission({
        submission: submissionFixture(),
        packageInfo: {
          path: "C:/build/gitwink.msixbundle",
          name: "gitwink.msixbundle",
          extension: ".msixbundle",
        },
        listingPlan,
      }),
    ).toThrow(
      "Store listing ja-jp does not exist; create its localized text before syncing screenshots",
    );
  });

  it("stages exactly the package, localized PNG, and patched metadata", async () => {
    const { root, manifestPath } = await listingFixture();
    const packagePath = path.join(root, "gitwink_0.12.0.msixbundle");
    const submissionPath = path.join(root, "submission.json");
    const outputPath = path.join(root, "prepared");
    await writeFile(packagePath, "bundle", "utf8");
    await writeFile(
      submissionPath,
      JSON.stringify(submissionFixture()),
      "utf8",
    );

    const result = await stageStoreSubmission(
      manifestPath,
      submissionPath,
      packagePath,
      outputPath,
    );
    expect(
      await readFile(
        path.join(result.stagingDirectory, result.packageName),
        "utf8",
      ),
    ).toBe("bundle");
    const metadata = JSON.parse(await readFile(result.metadataPath, "utf8"));
    const pendingImage = metadata.listings["en-us"].baseListing.images.find(
      (image) => image.fileStatus === "PendingUpload",
    );
    expect(
      await readFile(
        path.join(result.stagingDirectory, ...pendingImage.fileName.split("/")),
      ),
    ).toEqual(pngHeader(1366, 768));
  });
});
