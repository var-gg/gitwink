// Lane assignment for a DAG of commits, sorted newest-first.
//
// Algorithm: each lane is a slot that "expects" a particular parent SHA next.
// Walking newest-to-oldest:
//   1. If any open lane expects the current commit's SHA, that's our lane.
//      Otherwise allocate the smallest unused lane index.
//   2. For each parent of the commit:
//      - If an open lane already expects this parent (a merge converging
//        onto an existing line), reuse it.
//      - Else the first parent inherits the current commit's lane (the
//        "trunk" continues), and additional parents (merge commits) get
//        new lane allocations.
// We also emit edges from each commit to each of its parents so the SVG
// drawer can connect them with lines.

import type { CommitSummary } from "../types";

export interface LaneCommit {
  commit: CommitSummary;
  lane: number;
  color: string;
}

export interface LaneEdge {
  fromIdx: number; // index of the child commit in input order
  toIdx: number; // index of the parent (-1 if parent is outside the window)
  fromLane: number;
  toLane: number;
  color: string;
  /** Hidden commits sit between child and parent (an author-filtered
   * view bridged over them) — drawn dashed. */
  bridged: boolean;
}

/** A (possibly rewritten) parent link. `hash: null` = the ancestry leaves
 *  the rendered window entirely; the lane just runs off the bottom. */
export interface ParentLink {
  hash: string | null;
  bridged: boolean;
}

export interface LaneGraphData {
  laneCommits: LaneCommit[];
  edges: LaneEdge[];
  totalLanes: number;
}

export function computeLanes(
  commits: CommitSummary[],
  colorForCommit: (c: CommitSummary) => string,
  // Parent links per commit — defaults to the commit's real parents.
  // An author-filtered view passes bridged links (see lib/bridge.ts) so
  // the walk never waits on a commit the filter removed.
  parentsOf: (c: CommitSummary) => ParentLink[] = (c) =>
    c.parents.map((hash) => ({ hash, bridged: false })),
): LaneGraphData {
  const indexOf = new Map<string, number>();
  commits.forEach((c, i) => indexOf.set(c.hash, i));

  const openLanes: (string | null)[] = [];

  function takeFreeLane(): number {
    for (let i = 0; i < openLanes.length; i++) {
      if (openLanes[i] == null) return i;
    }
    openLanes.push(null);
    return openLanes.length - 1;
  }

  const laneCommits: LaneCommit[] = [];
  const edges: LaneEdge[] = [];

  commits.forEach((c, idx) => {
    // 1. Find or allocate our lane.
    let myLane = openLanes.findIndex((h) => h === c.hash);
    if (myLane === -1) {
      myLane = takeFreeLane();
    } else {
      openLanes[myLane] = null;
    }

    const color = colorForCommit(c);
    laneCommits.push({ commit: c, lane: myLane, color });

    // 2. Resolve a lane for each parent.
    parentsOf(c).forEach((link, pi) => {
      const parentHash = link.hash;
      const parentIdx =
        parentHash != null ? indexOf.get(parentHash) ?? -1 : -1;
      // A null hash must never match the empty (null) lane slots — only
      // look up a reusable lane for a real parent hash.
      let parentLane =
        parentHash != null
          ? openLanes.findIndex((h) => h === parentHash)
          : -1;
      if (parentLane === -1) {
        if (pi === 0) {
          parentLane = myLane;
        } else {
          parentLane = takeFreeLane();
        }
        // Off-window links occupy their lane with a sentinel no commit
        // hash can equal, so the lane visibly runs off the bottom without
        // ever being matched.
        openLanes[parentLane] = parentHash ?? `\u0000off:${idx}:${pi}`;
      }
      edges.push({
        fromIdx: idx,
        toIdx: parentIdx,
        fromLane: myLane,
        toLane: parentLane,
        color,
        bridged: link.bridged,
      });
    });
  });

  let totalLanes = 0;
  for (const lc of laneCommits) {
    if (lc.lane + 1 > totalLanes) totalLanes = lc.lane + 1;
  }
  for (const e of edges) {
    if (e.fromLane + 1 > totalLanes) totalLanes = e.fromLane + 1;
    if (e.toLane + 1 > totalLanes) totalLanes = e.toLane + 1;
  }

  return { laneCommits, edges, totalLanes };
}
