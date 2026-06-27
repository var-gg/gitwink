import { describe, expect, it } from "vitest";

import { parseDiff } from "./diff";
import { flattenDiff, longestLines } from "./diffView";
import { BASE_SBS_HEADER_H, BASE_SBS_LINE_H } from "./settings";

/** Build a flat item list straight from unified-diff text. */
function flatten(unified: string) {
  return flattenDiff(parseDiff(unified).hunks);
}

const SIMPLE = [
  "@@ -1,3 +1,3 @@",
  " ctx",
  "-old line",
  "+new line",
  " tail",
].join("\n");

describe("flattenDiff", () => {
  it("flattens header + rows into one ordered list", () => {
    const { items } = flatten(SIMPLE);
    expect(items.map((i) => i.kind)).toEqual([
      "header",
      "row", // ctx
      "row", // old/new paired replace
      "row", // tail
    ]);
    expect(items[0]).toMatchObject({ kind: "header", text: "@@ -1,3 +1,3 @@" });
    // The -/+ pair becomes one side-by-side replace row.
    const replace = items[2];
    expect(replace.kind === "row" && replace.left.text).toBe("old line");
    expect(replace.kind === "row" && replace.right.text).toBe("new line");
  });

  it("emits one coalesced 'change' segment for the replace", () => {
    const { segments } = flatten(SIMPLE);
    expect(segments).toHaveLength(1);
    expect(segments[0].type).toBe("change");
    // The single replace row sits at item index 2 (after header + ctx).
    const totalPx = BASE_SBS_HEADER_H + 3 * BASE_SBS_LINE_H;
    const top = (BASE_SBS_HEADER_H + BASE_SBS_LINE_H) / totalPx; // ctx then change
    expect(segments[0].topPct).toBeCloseTo(top * 100, 5);
    expect(segments[0].heightPct).toBeCloseTo((BASE_SBS_LINE_H / totalPx) * 100, 5);
  });

  it("coalesces a run of consecutive adds into a single tall segment", () => {
    const unified = [
      "@@ -1,1 +1,4 @@",
      " ctx",
      "+a",
      "+b",
      "+c",
    ].join("\n");
    const { items, segments } = flatten(unified);
    // header, ctx, +a, +b, +c
    expect(items).toHaveLength(5);
    expect(segments).toHaveLength(1);
    expect(segments[0].type).toBe("add");
    const totalPx = BASE_SBS_HEADER_H + 4 * BASE_SBS_LINE_H;
    // run spans 3 line rows starting after header+ctx
    expect(segments[0].heightPct).toBeCloseTo((3 * BASE_SBS_LINE_H / totalPx) * 100, 5);
  });

  it("splits segments across a hunk boundary (header breaks the run)", () => {
    const unified = [
      "@@ -1,1 +1,1 @@",
      "-x",
      "+y",
      "@@ -5,1 +5,1 @@",
      "-p",
      "+q",
    ].join("\n");
    const { segments } = flatten(unified);
    expect(segments).toHaveLength(2);
    expect(segments.every((s) => s.type === "change")).toBe(true);
  });

  it("returns no items / no segments for empty input", () => {
    const { items, segments } = flatten("");
    expect(items).toHaveLength(0);
    expect(segments).toHaveLength(0);
  });

  it("context-only diff produces no change segments", () => {
    const { segments } = flatten(["@@ -1,2 +1,2 @@", " a", " b"].join("\n"));
    expect(segments).toHaveLength(0);
  });
});

describe("longestLines", () => {
  it("tracks the widest text per side; header counts toward the left", () => {
    const { items } = flatten(SIMPLE);
    const { probeL, probeR } = longestLines(items);
    // The @@ header is the widest left-column string here.
    expect(probeL).toBe("@@ -1,3 +1,3 @@");
    expect(probeR).toBe("new line");
  });

  it("falls back to a single space so the probe is never empty", () => {
    const { probeL, probeR } = longestLines([]);
    expect(probeL).toBe(" ");
    expect(probeR).toBe(" ");
  });
});
