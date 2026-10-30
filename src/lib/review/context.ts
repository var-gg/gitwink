import type { FilePatch } from "../../types";
import type { ReviewSnapshot } from "./ipc";
import { boundedContext, CONTEXT_BYTE_BUDGET, patchForClipboard, utf8Bytes } from "../contextBudget";

/** Injectable reader keeps the byte/omission policy testable without native IPC. */
export async function buildReviewContext(snapshot: ReviewSnapshot, selected: Set<string>,
  read: (id: string) => Promise<FilePatch>): Promise<string> {
  const files = snapshot.files.filter(f => selected.has(f.id));
  const lines = ["# gitwink work review", `Repository: ${snapshot.repoName}`,
    `Mode: ${snapshot.mode}`, `Branch: ${snapshot.branch}`,
    `Base: ${snapshot.base ?? "empty tree"}`, `Head: ${snapshot.head ?? "unborn HEAD"}`,
    `Snapshot: ${snapshot.id} at ${new Date(snapshot.createdAt * 1000).toISOString()}`,
    ...(snapshot.fingerprint ? [`Index/worktree content fingerprint: ${snapshot.fingerprint}`] : []),
    `Selected: ${files.length}/${snapshot.files.length} changed files`, "", "## Selected files"];
  let listed = 0;
  let used = utf8Bytes(lines.join("\n"));
  for (const f of files) {
    const line = `- [${f.group}/${f.status}] ${JSON.stringify(f.path)}${f.oldPath ? ` (from ${JSON.stringify(f.oldPath)})` : ""} +${f.insertions} -${f.deletions}; bytes ${f.oldSize ?? "absent"} → ${f.newSize ?? "absent"}${f.isBinary ? " [binary omitted]" : ""}`;
    if (used + utf8Bytes(line) > 16 * 1024) break;
    lines.push(line); used += utf8Bytes(line) + 1; listed++;
  }
  if (listed < files.length) lines.push(`[OMITTED: ${files.length - listed} selected file descriptions exceed metadata budget]`);
  if (files.length < snapshot.files.length) lines.push(`[OMITTED: ${snapshot.files.length - files.length} files were not selected]`);
  lines.push("", "## Patches");
  used = utf8Bytes(lines.join("\n"));
  let loaded = 0;
  for (const f of files) {
    if (f.isBinary) continue;
    if (loaded >= 32 || f.insertions + f.deletions > 1200) {
      const notice = `[OMITTED: ${JSON.stringify(f.path)} exceeds patch count/line budget]`;
      if (used + utf8Bytes(notice) > CONTEXT_BYTE_BUDGET - 512) break;
      lines.push(notice); used += utf8Bytes(notice) + 1; continue;
    }
    const patch = patchForClipboard(await read(f.id));
    if (used + utf8Bytes(patch) > CONTEXT_BYTE_BUDGET - 512) {
      lines.push(`[OMITTED: ${JSON.stringify(f.path)} and remaining patches exceed byte budget]`); break;
    }
    lines.push(patch); used += utf8Bytes(patch) + 1; loaded++;
  }
  return boundedContext(lines.join("\n"));
}
