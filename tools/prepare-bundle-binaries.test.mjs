import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
  prepareUniversalCompanion,
  UNIVERSAL_MACOS_TARGET,
} from "./prepare-bundle-binaries.mjs";

describe("prepareUniversalCompanion", () => {
  it("does nothing outside a universal macOS bundle", () => {
    const run = vi.fn();
    const makeDirectory = vi.fn();

    expect(
      prepareUniversalCompanion({
        platform: "win32",
        targetTriple: "x86_64-pc-windows-msvc",
        run,
        makeDirectory,
      }),
    ).toBe(false);
    expect(run).not.toHaveBeenCalled();
    expect(makeDirectory).not.toHaveBeenCalled();
  });

  it("combines and verifies the two binaries after Tauri builds them", () => {
    const repositoryRoot = join("workspace", "gitwink");
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
    const run = vi.fn();
    const makeDirectory = vi.fn();
    const makeExecutable = vi.fn();

    expect(
      prepareUniversalCompanion({
        platform: "darwin",
        targetTriple: UNIVERSAL_MACOS_TARGET,
        repositoryRoot,
        exists: () => true,
        run,
        makeDirectory,
        makeExecutable,
        log: vi.fn(),
      }),
    ).toBe(true);
    expect(makeDirectory).toHaveBeenCalledWith(join(targetRoot, UNIVERSAL_MACOS_TARGET, "release"));
    expect(run).toHaveBeenNthCalledWith(1, "lipo", [
      "-create",
      arm64Binary,
      x64Binary,
      "-output",
      outputBinary,
    ]);
    expect(run).toHaveBeenNthCalledWith(2, "lipo", [
      outputBinary,
      "-verify_arch",
      "arm64",
      "x86_64",
    ]);
    expect(makeExecutable).toHaveBeenCalledWith(outputBinary);
  });

  it("fails before lipo when Tauri omitted an architecture", () => {
    const run = vi.fn();

    expect(() =>
      prepareUniversalCompanion({
        platform: "darwin",
        targetTriple: UNIVERSAL_MACOS_TARGET,
        repositoryRoot: join("workspace", "gitwink"),
        exists: (path) => !path.includes("x86_64-apple-darwin"),
        run,
      }),
    ).toThrow(/x86_64-apple-darwin/);
    expect(run).not.toHaveBeenCalled();
  });
});
