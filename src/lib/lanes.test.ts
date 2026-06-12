import { describe, expect, it } from "vitest";

import { bridgeParents } from "./bridge";
import { computeLanes } from "./lanes";
import type { CommitSummary } from "../types";

function mk(hash: string, parents: string[], author = "x"): CommitSummary {
  return {
    repoPath: "/r",
    repoName: "r",
    hash,
    shortHash: hash.slice(0, 7),
    summary: hash,
    author,
    email: `${author}@e`,
    timestamp: 0,
    branchLabel: null,
    isMerge: parents.length > 1,
    isTagged: false,
    parents,
    message: hash,
    remoteTipLabel: null,
    remoteTipExtraCount: 0,
  };
}

const color = () => "#000";

describe("computeLanes (default parents)", () => {
  it("threads a fork/merge through two lanes, tail running off-window", () => {
    // c0(main) and c1(feature) both fork from c2; c2's parent is below
    // the loaded window.
    const commits = [mk("c0", ["c2"]), mk("c1", ["c2"]), mk("c2", ["off"])];
    const g = computeLanes(commits, color);

    expect(g.laneCommits.map((lc) => lc.lane)).toEqual([0, 1, 0]);
    expect(g.totalLanes).toBe(2);
    // Tail edge: parent outside the window → toIdx -1, same lane, solid.
    const tail = g.edges.find((e) => e.fromIdx === 2)!;
    expect(tail.toIdx).toBe(-1);
    expect(tail.toLane).toBe(0);
    expect(g.edges.every((e) => !e.bridged)).toBe(true);
  });
});

describe("computeLanes + bridgeParents (author-filtered view)", () => {
  it("keeps the keymall scenario in one lane instead of a staircase", () => {
    // The reported breakage: an author's own commits alternating with
    // merge commits whose second parents (other authors' work) are
    // filtered out. Raw parents leak one lane per merge; bridged links
    // must keep the whole chain in a single lane.
    const all = [
      mk("m0", ["m1", "o0"]),
      mk("o0", ["o1"], "y"),
      mk("m1", ["m2", "o1"]),
      mk("o1", ["o2"], "y"),
      mk("m2", ["c3", "o2"]),
      mk("o2", ["off"], "y"),
      mk("c3", ["off2"]),
    ];
    const rows = all.filter((c) => c.author === "x");
    const effective = bridgeParents(all, (c) => c.author === "x");
    const g = computeLanes(rows, color, (c) => effective.get(c.hash) ?? []);

    expect(g.totalLanes).toBe(1);
    expect(g.laneCommits.map((lc) => lc.lane)).toEqual([0, 0, 0, 0]);
    // The only off-window line is the true tail (c3 → off2), kept solid.
    const offEdges = g.edges.filter((e) => e.toIdx === -1);
    expect(offEdges).toHaveLength(1);
    expect(offEdges[0].fromIdx).toBe(3);
    expect(offEdges[0].bridged).toBe(false);
  });

  it("draws bridged links dashed and reuses their lanes", () => {
    // a(x) → h(y) → c(x): the bridged edge must close lane 0 so c lands
    // back on it (no leak).
    const all = [mk("a", ["h"]), mk("h", ["c"], "y"), mk("c", [])];
    const rows = all.filter((c) => c.author === "x");
    const effective = bridgeParents(all, (c) => c.author === "x");
    const g = computeLanes(rows, color, (c) => effective.get(c.hash) ?? []);

    expect(g.totalLanes).toBe(1);
    expect(g.edges).toHaveLength(1);
    expect(g.edges[0]).toMatchObject({
      fromIdx: 0,
      toIdx: 1,
      fromLane: 0,
      toLane: 0,
      bridged: true,
    });
  });

  it("never matches an off-window marker against a free lane slot", () => {
    // Two isolated visible commits whose chains both leave the window:
    // each keeps its own lane + dashed tail line; the null-hash marker
    // must not be "found" in an empty (null) lane slot.
    const all = [
      mk("a", ["h1"]),
      mk("h1", ["off"], "y"),
      mk("b", ["h2"]),
      mk("h2", ["off2"], "y"),
    ];
    const rows = all.filter((c) => c.author === "x");
    const effective = bridgeParents(all, (c) => c.author === "x");
    const g = computeLanes(rows, color, (c) => effective.get(c.hash) ?? []);

    expect(g.laneCommits.map((lc) => lc.lane)).toEqual([0, 1]);
    const markers = g.edges.filter((e) => e.toIdx === -1);
    expect(markers).toHaveLength(2);
    expect(markers.every((e) => e.bridged)).toBe(true);
    expect(new Set(markers.map((e) => e.toLane)).size).toBe(2);
  });
});
