import { expect, it } from "vitest";
import { boundedContext, patchForClipboard, utf8Bytes } from "./contextBudget";

it("limits UTF-8 bytes including the omission notice, without broken characters", () => {
  const result = boundedContext("한글😀".repeat(10000), 1000);
  expect(utf8Bytes(result)).toBeLessThanOrEqual(1000);
  expect(result).not.toContain("�");
  expect(result).toContain("OMITTED");
});

it("copies exact old and new references and explicit truncation", () => {
  const text = patchForClipboard({ text: "@@ -1 +1 @@\n", truncated: true,
    reason: "patch_byte_limit", oldPath: "before", newPath: "after",
    oldRevision: "base", newRevision: "head" });
  expect(text).toContain("base:before → head:after");
  expect(text).toContain("incomplete");
});
