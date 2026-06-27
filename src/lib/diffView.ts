// Diff view-model: pure transforms from parsed hunks into what the
// side-by-side renderer needs — a flat, virtualizable row list, the overview
// rail's coloured segments, and the per-side longest line for the width probe.
// No React, no DOM, so it's unit-tested directly (diffView.test.ts).

import type { DiffHunk, DiffSide } from "./diff";
import { BASE_SBS_HEADER_H, BASE_SBS_LINE_H } from "./settings";

/** One flattened visual row: a hunk header, or one side-by-side line. Both
 * diff columns render the SAME item list (header text on the left, a blank
 * header on the right), so their fixed-height rows stay aligned by
 * construction — which is what lets each column virtualize independently. */
export type Item =
  | { kind: "header"; text: string }
  | { kind: "row"; left: DiffSide; right: DiffSide };

/** Overview-rail mark: a coloured band over a run of changed rows, positioned
 * as a percentage of the diff's total pixel height. */
export interface MinimapSegment {
  /** distance from the top of the diff, as a percentage of total height */
  topPct: number;
  /** segment span, as a percentage of total height */
  heightPct: number;
  type: "add" | "delete" | "change";
}

/** Flatten parsed hunks into one virtualizable item list, and compute the
 * overview-rail segments at the same time. Segment positions are pixel-exact
 * fractions (a header is a touch taller than a line) using the BASE row
 * heights — the ratio is scale-independent, so the marks line up with the
 * scroll thumb at any UI scale. Consecutive changed rows of the same kind
 * coalesce into one band, so a 200-line block is a single tall mark. */
export function flattenDiff(hunks: DiffHunk[]): {
  items: Item[];
  segments: MinimapSegment[];
} {
  const items: Item[] = [];
  for (const h of hunks) {
    items.push({ kind: "header", text: h.header });
    for (const r of h.rows) {
      items.push({ kind: "row", left: r.left, right: r.right });
    }
  }

  // Cumulative base-px tops (header vs line height) for exact mark placement.
  const tops = new Array<number>(items.length + 1);
  tops[0] = 0;
  for (let i = 0; i < items.length; i++) {
    tops[i + 1] =
      tops[i] +
      (items[i].kind === "header" ? BASE_SBS_HEADER_H : BASE_SBS_LINE_H);
  }
  const totalPx = tops[items.length] || 1;

  const segments: MinimapSegment[] = [];
  let cur: { start: number; end: number; type: MinimapSegment["type"] } | null =
    null;
  const flush = () => {
    if (!cur) return;
    segments.push({
      topPct: (tops[cur.start] / totalPx) * 100,
      heightPct: ((tops[cur.end] - tops[cur.start]) / totalPx) * 100,
      type: cur.type,
    });
    cur = null;
  };
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (it.kind === "row") {
      const hasDel = it.left.type === "delete";
      const hasAdd = it.right.type === "add";
      if (hasDel || hasAdd) {
        const t: MinimapSegment["type"] =
          hasDel && hasAdd ? "change" : hasAdd ? "add" : "delete";
        if (cur && cur.end === i && cur.type === t) cur.end = i + 1;
        else {
          flush();
          cur = { start: i, end: i + 1, type: t };
        }
      } else {
        flush();
      }
    } else {
      flush(); // a hunk header breaks a run of changes
    }
  }
  flush();
  return { items, segments };
}

/** Longest line per side — rendered as a hidden in-flow probe so each column's
 * track keeps a stable horizontal width as the vertical window slides (only a
 * handful of rows are mounted, so without this the h-scrollbar would jump).
 * The @@ header sits in the left column, so it counts toward the left probe. */
export function longestLines(items: Item[]): { probeL: string; probeR: string } {
  let l = "";
  let r = "";
  for (const it of items) {
    if (it.kind === "row") {
      if (it.left.text.length > l.length) l = it.left.text;
      if (it.right.text.length > r.length) r = it.right.text;
    } else if (it.text.length > l.length) {
      l = it.text;
    }
  }
  return { probeL: l || " ", probeR: r || " " };
}
