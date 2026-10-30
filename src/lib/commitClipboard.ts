// Shared commit clipboard helpers — used by both the all-repos windowed
// timeline and the single-repo timeline so the "copy" affordances stay
// identical across modes.

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import type { MenuItem } from "../components/ContextMenu";
import type { CommitSummary } from "../types";
import { boundedContext, patchForClipboard, utf8Bytes, CONTEXT_BYTE_BUDGET } from "./contextBudget";
import { buildAiContext } from "./copy";
import { changedFiles, fileDiff, openFileHistory } from "./ipc";
import { refLine, refLineWithFile } from "./smartcopy";

/** Diff-size ceiling (changed lines) under which "Copy as AI context"
 * inlines the full patch; bigger commits get a file-list summary only so
 * the clipboard payload stays usable in a chat prompt. */
const AI_CONTEXT_DIFF_LINE_BUDGET = 800;

export interface CommitMenuLabels {
  copy: string;
  copyWithReference: string;
  showFileHistory: string;
  showFileHistoryAria: string;
  copyFilePath: string;
  copyAiContext: string;
  copyCommitMessage: string;
  copyShortHash: string;
  copyFullHash: string;
}

const ENGLISH_MENU_LABELS: CommitMenuLabels = {
  copy: "Copy",
  copyWithReference: "Copy with reference",
  showFileHistory: "🕘 Show file history",
  showFileHistoryAria: "Show file history",
  copyFilePath: "Copy file path",
  copyAiContext: "Copy as AI context",
  copyCommitMessage: "Copy commit message",
  copyShortHash: "Copy short hash",
  copyFullHash: "Copy full hash",
};

/** Copy a commit as an AI-ready context block: the changed-file list plus,
 * when the commit is small enough, the full diff. Returns a status string
 * for the caller's transient UI ("Copied ✓" / "Copy failed"). */
export async function copyCommitAiContext(
  commit: CommitSummary,
): Promise<"copied" | "error"> {
  try {
    const { files, total } = await changedFiles(commit.repoPath, commit.hash);
    let diffText: string | null = null;
    const totalLines = files.reduce(
      (acc, f) => acc + (f.isBinary ? 0 : f.insertions + f.deletions),
      0,
    );
    if (totalLines <= AI_CONTEXT_DIFF_LINE_BUDGET) {
      try {
        const parts: string[] = [];
        let used = 0;
        for (const f of files) {
          if (f.isBinary) { parts.push(`[OMITTED: binary content ${f.path}]`); continue; }
          const t = await fileDiff(commit.repoPath, commit.hash, f.path);
          const part = patchForClipboard(t);
          if (used + utf8Bytes(part) > CONTEXT_BYTE_BUDGET / 2) {
            parts.push(`[OMITTED: ${f.path} and remaining patches exceed byte budget]`);
            break;
          }
          parts.push(part);
          used += utf8Bytes(part);
        }
        diffText = parts.join("\n");
      } catch {
        diffText = "[OMITTED: patch could not be loaded]";
      }
    }
    if (diffText == null) diffText = "[OMITTED: patches exceed changed-line budget]";
    let text = buildAiContext(commit, files, diffText);
    // The backend caps huge file lists (vendor-bump commits) — an AI-context
    // copy must SAY so, never silently pretend the list is complete.
    if (total > files.length) {
      text += `\n(file list truncated: showing ${files.length} of ${total} changed files)\n`;
    }
    await writeText(boundedContext(text));
    return "copied";
  } catch {
    return "error";
  }
}

/** Context-menu items scoped to a single changed file: open its history in
 * the panel (cross-window) and copy its path. Shared by the timelines AND the
 * diff window so right-clicking a file behaves identically everywhere. The
 * history entry needs a repo to resolve against, so it's omitted when the
 * caller has a path but no repo. */
export function fileMenuItems(
  repoPath: string | null,
  filePath: string | null,
  labels: CommitMenuLabels = ENGLISH_MENU_LABELS,
): MenuItem[] {
  if (!filePath) return [];
  const items: MenuItem[] = [];
  if (repoPath) {
    items.push({
      label: labels.showFileHistory,
      ariaLabel: labels.showFileHistoryAria,
      onClick: () => void openFileHistory(repoPath, filePath),
    });
  }
  items.push({
    label: labels.copyFilePath,
    onClick: () => void writeText(filePath),
  });
  return items;
}

/** Build the timeline context-menu items for a (possibly null) commit, an
 * optional changed-file path, and the current text selection. Shared by the
 * all-repos and single-repo timelines so their right-click menus match.
 *
 * Grouped by subject — selection · file · commit — each its own section so the
 * one real action (file history) reads as an action, not a clipboard verb. */
export function buildCommitMenuItems(opts: {
  commit: CommitSummary | null;
  filePath: string | null;
  selection: string;
  onCopyAiContext: (commit: CommitSummary) => void;
  labels?: CommitMenuLabels;
}): MenuItem[] {
  const {
    commit,
    filePath,
    selection,
    onCopyAiContext,
    labels = ENGLISH_MENU_LABELS,
  } = opts;
  const items: MenuItem[] = [];

  if (selection) {
    items.push({ label: labels.copy, onClick: () => void writeText(selection) });
    if (commit) {
      const ref = filePath
        ? refLineWithFile(commit.repoName, commit.shortHash, filePath, null, null)
        : refLine(commit.repoName, commit.shortHash);
      items.push({
        label: labels.copyWithReference,
        onClick: () => void writeText(`${ref}\n${selection}`),
      });
    }
    items.push({ divider: true });
  }

  const fileItems = fileMenuItems(commit?.repoPath ?? null, filePath, labels);
  if (fileItems.length) {
    items.push(...fileItems);
    if (commit) items.push({ divider: true });
  }

  if (commit) {
    items.push({
      label: labels.copyAiContext,
      onClick: () => onCopyAiContext(commit),
    });
    const messageText = (commit.message || commit.summary).trim();
    if (messageText) {
      items.push({
        label: labels.copyCommitMessage,
        onClick: () => void writeText(messageText),
      });
    }
    items.push({
      label: labels.copyShortHash,
      onClick: () => void writeText(commit.shortHash),
    });
    items.push({
      label: labels.copyFullHash,
      onClick: () => void writeText(commit.hash),
    });
  }

  return items;
}
