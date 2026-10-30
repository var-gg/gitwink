
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const REQUIRED_TARGETS = new Map([
  ["windows-x86_64-msi", /\.msi$/i],
  ["windows-x86_64-nsis", /-setup\.exe$/i],
  ["darwin-aarch64-app", /\.app\.tar\.gz$/i],
  ["darwin-x86_64-app", /\.app\.tar\.gz$/i],
]);

/** Validate the update-routing contract shared by Tauri and release.yml. */
export function validateUpdateManifest(manifest, expectedVersion) {
  const errors = [];
  if (manifest?.version !== expectedVersion) {
    errors.push(
      `version is ${JSON.stringify(manifest?.version)}; expected ${JSON.stringify(expectedVersion)}`,
    );
  }

  const platforms = manifest?.platforms;
  if (platforms == null || typeof platforms !== "object") {
    return [...errors, "platforms must be an object"];
  }

  for (const [target, artifactPattern] of REQUIRED_TARGETS) {
    const entry = platforms[target];
    if (entry == null || typeof entry !== "object") {
      errors.push(`missing ${target}`);
      continue;
    }
    if (typeof entry.signature !== "string" || entry.signature.trim() === "") {
      errors.push(`${target} has no signature`);
    }
    if (typeof entry.url !== "string") {
      errors.push(`${target} has no URL`);
      continue;
    }
    let url;
    try {
      url = new URL(entry.url);
    } catch {
      errors.push(`${target} URL is invalid`);
      continue;
    }
    const expectedTagPath = `/releases/download/v${expectedVersion}/`;
    if (!url.pathname.includes(expectedTagPath)) {
      errors.push(`${target} URL does not belong to v${expectedVersion}`);
    }
    if (!artifactPattern.test(url.pathname)) {
      errors.push(`${target} points at the wrong installer family`);
    }
  }

  return errors;
}

async function main() {
  const [manifestPath, expectedVersion] = process.argv.slice(2);
  if (!manifestPath || !expectedVersion) {
    throw new Error(
      "usage: node tools/verify-update-manifest.mjs <latest.json> <version>",
    );
  }
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const errors = validateUpdateManifest(manifest, expectedVersion);
  if (errors.length > 0) {
    throw new Error(`invalid updater manifest:\n- ${errors.join("\n- ")}`);
  }
  console.log(
    `Updater manifest ${expectedVersion} preserves MSI, NSIS and universal macOS routes.`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
