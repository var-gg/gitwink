// Parse unified diff (git's standard patch output) into side-by-side rows.
//
// We pair adjacent `-` and `+` lines as "replace" rows so the user sees the
// old and new text side by side. Unmatched deletes go on the left only;
// unmatched adds go on the right only. Context rows mirror both sides.
//
// Not a true LCS — when a hunk reorders lines we won't align them perfectly.
// Good enough for the typical "edited a function" case.

export type DiffLineType = "delete" | "add" | "context" | null;

export interface DiffSide {
  lineNum: number | null;
  text: string;
  type: DiffLineType;
}

export interface DiffRow {
  left: DiffSide;
  right: DiffSide;
}

export interface DiffHunk {
  header: string;
  rows: DiffRow[];
}

const BLANK: DiffSide = { lineNum: null, text: "", type: null };

export function parseDiff(unified: string): { hunks: DiffHunk[] } {
  const lines = unified.split("\n");
  const hunks: DiffHunk[] = [];
  let cur: DiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;
  let pendingDeletes: { lineNum: number; text: string }[] = [];

  let deleteCursor = 0;

  function flushDeletes() {
    if (!cur) return;
    while (deleteCursor < pendingDeletes.length) {
      const d = pendingDeletes[deleteCursor++];
      cur.rows.push({
        left: { lineNum: d.lineNum, text: d.text, type: "delete" },
        right: BLANK,
      });
    }
    pendingDeletes = [];
    deleteCursor = 0;
  }

  for (const raw of lines) {
    if (raw.startsWith("@@")) {
      flushDeletes();
      if (cur) hunks.push(cur);
      const m = raw.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)?/);
      if (m) {
        oldLine = parseInt(m[1], 10);
        newLine = parseInt(m[2], 10);
      }
      cur = { header: raw, rows: [] };
      pendingDeletes = [];
      continue;
    }
    // A new file section ends the current hunk, so the headers that follow
    // (index/---/+++) are preamble again — never hunk content.
    if (raw.startsWith("diff ")) {
      flushDeletes();
      if (cur) hunks.push(cur);
      cur = null;
      continue;
    }
    if (!cur) continue;
    // NOTE: "--- "/"+++ " are deliberately NOT skipped in-hunk — a deleted
    // line whose content starts with "-- " (an SQL/Lua/Haskell comment)
    // arrives as "--- …" and an added "++ " line as "+++ …"; skipping them
    // silently dropped the line and shifted every later line number. The
    // real ---/+++ preamble only occurs while `cur` is null (above).
    if (raw.startsWith("index ") || raw.startsWith("\\ ")) {
      continue;
    }
    const origin = raw.charAt(0);
    const text = raw.slice(1);
    if (origin === "-") {
      pendingDeletes.push({ lineNum: oldLine, text });
      oldLine++;
    } else if (origin === "+") {
      if (deleteCursor < pendingDeletes.length) {
        const d = pendingDeletes[deleteCursor++];
        cur.rows.push({
          left: { lineNum: d.lineNum, text: d.text, type: "delete" },
          right: { lineNum: newLine, text, type: "add" },
        });
      } else {
        cur.rows.push({
          left: BLANK,
          right: { lineNum: newLine, text, type: "add" },
        });
      }
      newLine++;
    } else if (origin === " ") {
      flushDeletes();
      cur.rows.push({
        left: { lineNum: oldLine, text, type: "context" },
        right: { lineNum: newLine, text, type: "context" },
      });
      oldLine++;
      newLine++;
    }
  }
  flushDeletes();
  if (cur) hunks.push(cur);
  return { hunks };
}
