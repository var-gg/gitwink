import { describe, expect, it } from "vitest";

import type { CommitSummary } from "../types";
import {
  mergeAnchoredCommit,
  shouldApplyAnchoredRow,
  shouldRestoreAnchorExpansion,
} from "./controlHandoff";

function commit(hash: string, timestamp: number, repoPath = "C:\\repo"): CommitSummary {
  return {
    repoPath,
    repoName: "repo",
    hash,
    shortHash: hash.slice(0, 7),
    summary: hash,
    author: "Agent",
    email: "agent@example.com",
    timestamp,
    branchLabel: null,
    isMerge: false,
    isTagged: false,
    parents: [],
    message: hash,
    remoteTipLabel: null,
    remoteTipExtraCount: 0,
  };
}

describe("control handoff commit injection", () => {
  it("renders an exact target before the surrounding timeline finishes loading", () => {
    const target = commit("target", 10);
    expect(mergeAnchoredCommit(null, target, target.repoPath, false)).toEqual([
      target,
    ]);
  });

  it("inserts an older target without disturbing newest-first order", () => {
    const newest = commit("newest", 30);
    const middle = commit("middle", 20);
    const target = commit("target", 10);
    expect(
      mergeAnchoredCommit([newest, middle], target, "C:\\repo", false)?.map(
        (row) => row.hash,
      ),
    ).toEqual(["newest", "middle", "target"]);
  });

  it("preserves snapshot identity when the target is already loaded", () => {
    const target = commit("target", 10);
    const rows = [target];
    expect(mergeAnchoredCommit(rows, target, "C:\\repo", false)).toBe(rows);
  });

  it("never leaks a target into another repo or file-history scope", () => {
    const rows = [commit("visible", 20, "C:\\other")];
    const target = commit("target", 10);
    expect(mergeAnchoredCommit(rows, target, "C:\\other", false)).toBe(rows);
    expect(mergeAnchoredCommit(rows, target, "C:\\repo", true)).toBe(rows);
  });
});

describe("control handoff anchor ownership", () => {
  it("applies a newly delivered landing", () => {
    expect(shouldApplyAnchoredRow(2, 1, 0, 4, -1)).toBe(true);
  });

  it("re-centres when asynchronous surrounding rows move the target", () => {
    expect(shouldApplyAnchoredRow(2, 2, 2, 193, 0)).toBe(true);
  });

  it("does not steal the viewport back after user navigation retires tracking", () => {
    expect(shouldApplyAnchoredRow(2, 2, 0, 193, 0)).toBe(false);
  });

  it("does not repeat a stable landing", () => {
    expect(shouldApplyAnchoredRow(2, 2, 2, 193, 193)).toBe(false);
  });

  it("restores cold-mount effect replay while the landing owns navigation", () => {
    expect(shouldRestoreAnchorExpansion(2, 2, "target", true, null)).toBe(
      true,
    );
    expect(
      shouldRestoreAnchorExpansion(2, 2, "target", false, "previous"),
    ).toBe(true);
  });

  it("never restores presentation after user navigation retires ownership", () => {
    expect(shouldRestoreAnchorExpansion(2, 0, "target", true, null)).toBe(
      false,
    );
  });
});
