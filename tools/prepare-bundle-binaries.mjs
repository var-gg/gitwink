import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const UNIVERSAL_MACOS_TARGET = "universal-apple-darwin";

const toolDirectory = dirname(fileURLToPath(import.meta.url));
const defaultRepositoryRoot = resolve(toolDirectory, "..");

export function prepareUniversalCompanion({
  platform = process.platform,
  targetTriple = process.env.TAURI_ENV_TARGET_TRIPLE,
  repositoryRoot = defaultRepositoryRoot,
  exists = existsSync,
  makeDirectory = (path) => mkdirSync(path, { recursive: true }),
  run = (command, args) => execFileSync(command, args, { stdio: "inherit" }),
  makeExecutable = (path) => chmodSync(path, 0o755),
  log = console.log,
} = {}) {
  if (platform !== "darwin" || targetTriple !== UNIVERSAL_MACOS_TARGET) {
    return false;
  }

  const targetRoot = join(repositoryRoot, "src-tauri", "target");
  const arm64Binary = join(
    targetRoot,
    "aarch64-apple-darwin",
    "release",
    "gitwinkctl",
  );
  const x64Binary = join(
    targetRoot,
    "x86_64-apple-darwin",
    "release",
    "gitwinkctl",
  );
  const outputBinary = join(
    targetRoot,
    UNIVERSAL_MACOS_TARGET,
    "release",
    "gitwinkctl",
  );

  for (const binary of [arm64Binary, x64Binary]) {
    if (!exists(binary)) {
      throw new Error(`Tauri did not build companion binary: ${binary}`);
    }
  }

  makeDirectory(dirname(outputBinary));
  run("lipo", [
    "-create",
    arm64Binary,
    x64Binary,
    "-output",
    outputBinary,
  ]);
  run("lipo", [
    outputBinary,
    "-verify_arch",
    "arm64",
    "x86_64",
  ]);
  makeExecutable(outputBinary);
  log(`Prepared universal macOS companion binary: ${outputBinary}`);
  return true;
}

const invokedUrl = process.argv[1]
  ? pathToFileURL(resolve(process.argv[1])).href
  : null;
if (invokedUrl === import.meta.url) {
  prepareUniversalCompanion();
}
