import { describe, expect, it } from "vitest";

import {
  canReuseRepoCommitsSnapshot,
  reusableRepoCommitsFingerprint,
} from "./repoCommitsReuse";

describe("single-repo commit snapshot reuse", () => {
  it("reuses only a complete non-shallow snapshot", () => {
    expect(
      reusableRepoCommitsFingerprint("refs:v1", {
        incomplete: false,
        shallow: false,
      }),
    ).toBe("refs:v1");
  });

  it("forces a later retry after partial or shallow coverage", () => {
    expect(
      reusableRepoCommitsFingerprint("refs:v1", {
        incomplete: true,
        shallow: false,
      }),
    ).toBeNull();
    expect(
      reusableRepoCommitsFingerprint("refs:v1", {
        incomplete: false,
        shallow: true,
      }),
    ).toBeNull();
  });

  it("requires a non-empty unchanged fingerprint", () => {
    expect(canReuseRepoCommitsSnapshot("refs:v1", "refs:v1")).toBe(true);
    expect(canReuseRepoCommitsSnapshot(null, "refs:v1")).toBe(false);
    expect(canReuseRepoCommitsSnapshot("", "")).toBe(false);
  });
});
