import { expect, it } from "vitest";
import { buildReviewContext } from "./context";
import type { ReviewSnapshot, ReviewFile } from "./ipc";
import { CONTEXT_BYTE_BUDGET, utf8Bytes } from "../contextBudget";

const file = (id: string, binary = false): ReviewFile => ({ id, group: "range", path: id,
  oldPath: null, status: "modified", insertions: 1, deletions: 1, isBinary: binary, oldSize: 10, newSize: 11 });
const snapshot = (files: ReviewFile[]): ReviewSnapshot => ({ id: "snapshot-1", repoPath: "/repo",
  repoName: "repo", branch: "refs/heads/main", identity: "physical", mode: "range",
  base: "a".repeat(40), head: "b".repeat(40), liveHead: "b".repeat(40), baseline: null,
  baselineStatus: "missing", fingerprint: null, createdAt: 1000, files, canMarkReviewed: true });

it("copies only selected text, includes exact references and identifies excluded binary files", async () => {
  const s = snapshot([file("a.ts"), file("image.png", true), file("unselected.ts")]);
  const reads: string[] = [];
  const text = await buildReviewContext(s, new Set(["a.ts", "image.png"]), async id => {
    reads.push(id); return { text: "-before\n+after\n", truncated: false, reason: null,
      oldPath: id, newPath: id, oldRevision: s.base!, newRevision: s.head! };
  });
  expect(reads).toEqual(["a.ts"]);
  expect(text).toContain(s.base);
  expect(text).toContain(s.head);
  expect(text).toContain("binary omitted");
  expect(text).toContain("1 files were not selected");
});

it("stops oversized single-line patches and keeps the entire output within its byte budget", async () => {
  const s = snapshot([file("huge.ts"), file("later.ts")]);
  let reads = 0;
  const text = await buildReviewContext(s, new Set(s.files.map(f => f.id)), async id => {
    reads++; return { text: "+" + "한글".repeat(30000), truncated: false, reason: null,
      oldPath: id, newPath: id, oldRevision: s.base!, newRevision: s.head! };
  });
  expect(reads).toBe(1);
  expect(utf8Bytes(text)).toBeLessThanOrEqual(CONTEXT_BYTE_BUDGET);
  expect(text).toContain("remaining patches exceed byte budget");
});

it("propagates a stale snapshot error instead of copying mixed versions", async () => {
  const s = snapshot([file("a")]);
  await expect(buildReviewContext(s, new Set(["a"]), async () => { throw Error("snapshot changed"); }))
    .rejects.toThrow("snapshot changed");
});
