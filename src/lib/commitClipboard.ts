// Shared commit clipboard helpers — used by both the all-repos windowed
// timeline and the single-repo timeline so the "copy" affordances stay
// identical across modes.

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import type { MenuItem } from "../components/ContextMenu";
import type { CommitSummary } from "../types";
import { buildAiContext } from "./copy";
import { changedFiles, fileDiff } from "./ipc";
import { refLine, refLineWithFile } from "./smartcopy";

/** Diff-size ceiling (changed lines) under which "Copy as AI context"
 * inlines the full patch; bigger commits get a file-list summary only so
 * the clipboard payload stays usable in a chat prompt. */
const AI_CONTEXT_DIFF_LINE_BUDGET = 800;

/** Copy a commit as an AI-ready context block: the changed-file list plus,
 * when the commit is small enough, the full diff. Returns a status string
 * for the caller's transient UI ("Copied ✓" / "Copy failed"). */
export async function copyCommitAiContext(
  commit: CommitSummary,
): Promise<"copied" | "error"> {
  try {
    const files = await changedFiles(commit.repoPath, commit.hash);
    let diffText: string | null = null;
    const totalLines = files.reduce(
      (acc, f) => acc + (f.isBinary ? 0 : f.insertions + f.deletions),
      0,
    );
    if (!files.some((f) => f.isBinary) && totalLines <= AI_CONTEXT_DIFF_LINE_BUDGET) {
      try {
        const parts: string[] = [];
        for (const f of files) {
          const t = await fileDiff(commit.repoPath, commit.hash, f.path);
          parts.push(`--- ${f.path}\n${t}`);
        }
        diffText = parts.join("\n");
      } catch {
        diffText = null;
      }
    }
    await writeText(buildAiContext(commit, files, diffText));
    return "copied";
  } catch {
    return "error";
  }
}

/** Build the timeline context-menu items for a (possibly null) commit, an
 * optional changed-file path, and the current text selection. Shared by the
 * all-repos and single-repo timelines so their right-click menus match. */
export function buildCommitMenuItems(opts: {
  commit: CommitSummary | null;
  filePath: string | null;
  selection: string;
  onCopyAiContext: (commit: CommitSummary) => void;
}): MenuItem[] {
  const { commit, filePath, selection, onCopyAiContext } = opts;
  const items: MenuItem[] = [];

  if (selection) {
    items.push({ label: "Copy", onClick: () => void writeText(selection) });
    if (commit) {
      const ref = filePath
        ? refLineWithFile(commit.repoName, commit.shortHash, filePath, null, null)
        : refLine(commit.repoName, commit.shortHash);
      items.push({
        label: "Copy with reference",
        onClick: () => void writeText(`${ref}\n${selection}`),
      });
    }
    items.push({ divider: true });
  }

  if (filePath) {
    items.push({
      label: "Copy file path",
      onClick: () => void writeText(filePath),
    });
  }

  if (commit) {
    items.push({
      label: "Copy as AI context",
      onClick: () => onCopyAiContext(commit),
    });
    const messageText = (commit.message || commit.summary).trim();
    if (messageText) {
      items.push({
        label: "Copy commit message",
        onClick: () => void writeText(messageText),
      });
    }
    items.push({
      label: "Copy short hash",
      onClick: () => void writeText(commit.shortHash),
    });
    items.push({
      label: "Copy full hash",
      onClick: () => void writeText(commit.hash),
    });
  }

  return items;
}
