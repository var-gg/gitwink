import { describe, expect, it } from "vitest";

import { validateUpdateManifest } from "./verify-update-manifest.mjs";

function entry(name) {
  return {
    signature: "signed",
    url: `https://github.com/var-gg/gitwink/releases/download/v1.2.3/${name}`,
  };
}

describe("release updater manifest", () => {
  it("accepts installer-family-specific routes", () => {
    const manifest = {
      version: "1.2.3",
      platforms: {
        "windows-x86_64-msi": entry("gitwink_1.2.3_x64_en-US.msi"),
        "windows-x86_64-nsis": entry("gitwink_1.2.3_x64-setup.exe"),
        "darwin-aarch64-app": entry("gitwink_universal.app.tar.gz"),
        "darwin-x86_64-app": entry("gitwink_universal.app.tar.gz"),
      },
    };
    expect(validateUpdateManifest(manifest, "1.2.3")).toEqual([]);
  });

  it("rejects missing targets and cross-family Windows artifacts", () => {
    const manifest = {
      version: "1.2.3",
      platforms: {
        "windows-x86_64-msi": entry("gitwink_1.2.3_x64-setup.exe"),
      },
    };
    expect(validateUpdateManifest(manifest, "1.2.3")).toEqual(
      expect.arrayContaining([
        "windows-x86_64-msi points at the wrong installer family",
        "missing windows-x86_64-nsis",
        "missing darwin-aarch64-app",
        "missing darwin-x86_64-app",
      ]),
    );
  });
});
