// "Copy as AI context" — produce the markdown block from the spec and write
// it to the clipboard. The format is meant to be pasted into Claude / Codex
// / Cursor / Aider chat so the agent has full context of what it changed.

import type { ChangedFile, CommitSummary } from "../types";

function formatStatus(f: ChangedFile): string {
  switch (f.status) {
    case "new":
      return "[NEW]";
    case "renamed":
      return `[RENAMED from ${f.oldPath ?? "?"}]`;
    case "deleted":
      return "[DELETED]";
    case "copied":
      return "[COPIED]";
    case "typechange":
      return "[TYPE]";
    default:
      return "";
  }
}

function relTime(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = Math.max(0, now - unixSeconds);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86_400)}d ago`;
}

export function buildAiContext(
  commit: CommitSummary,
  files: ChangedFile[],
  diffText: string | null,
): string {
  const lines: string[] = [];
  lines.push(`## Commit: ${commit.summary}`);
  lines.push("");
  lines.push(`**Repo:** ${commit.repoName}  `);
  lines.push(`**Author:** ${commit.author}  `);
  lines.push(`**Hash:** ${commit.hash}  `);
  lines.push(`**Time:** ${relTime(commit.timestamp)}`);
  lines.push("");

  if (files.length > 0) {
    lines.push("### Changed files");
    for (const f of files) {
      const status = formatStatus(f);
      const stat = f.isBinary
        ? `(binary)`
        : `(+${f.insertions}, −${f.deletions})`;
      lines.push(
        `- \`${f.path}\` ${stat}${status ? ` ${status}` : ""}`.trim(),
      );
    }
    lines.push("");
  }

  if (diffText && diffText.trim().length > 0) {
    lines.push("### Diff");
    lines.push("```diff");
    lines.push(diffText.trimEnd());
    lines.push("```");
    lines.push("");
  }

  const message = (commit.message || "").trim();
  if (message && message !== commit.summary.trim()) {
    lines.push("### Commit message");
    lines.push(message);
    lines.push("");
  }

  return lines.join("\n");
}
