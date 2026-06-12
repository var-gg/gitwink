import { describe, expect, it } from "vitest";

import { bridgeParents } from "./bridge";
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

const byAuthor =
  (author: string) =>
  (c: CommitSummary): boolean =>
    c.author === author;

describe("bridgeParents", () => {
  it("keeps a direct visible parent unbridged", () => {
    const all = [mk("a", ["b"]), mk("b", [])];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("a")).toEqual([{ hash: "b", bridged: false }]);
    expect(eff.get("b")).toEqual([]);
  });

  it("bridges through hidden commits to the nearest visible ancestor", () => {
    // a(x) → h(y) → c(x): filtering to x must link a → c, dashed.
    const all = [mk("a", ["h"]), mk("h", ["c"], "y"), mk("c", [])];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("a")).toEqual([{ hash: "c", bridged: true }]);
    expect(eff.has("h")).toBe(false);
  });

  it("is order-independent (clock-skew rows do not break resolution)", () => {
    // Hidden commit listed BEFORE its child — resolution is hash-based.
    const all = [mk("h", ["v"], "y"), mk("c", ["h"]), mk("v", [])];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("c")).toEqual([{ hash: "v", bridged: true }]);
  });

  it("drops a hidden chain that exits the window when a visible parent exists", () => {
    // Merge: first parent visible, second parent's hidden chain leaves
    // the loaded list — no extra line, just the trunk link.
    const all = [mk("m", ["v", "h"]), mk("v", []), mk("h", ["off"], "y")];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("m")).toEqual([{ hash: "v", bridged: false }]);
  });

  it("emits one dashed off-window marker for an isolated visible commit", () => {
    // Every path from `a` leaves the window through hidden commits.
    const all = [mk("a", ["h"]), mk("h", ["off"], "y")];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("a")).toEqual([{ hash: null, bridged: true }]);
  });

  it("keeps the plain tail marker solid (direct off-window parent)", () => {
    const all = [mk("a", ["off"])];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("a")).toEqual([{ hash: null, bridged: false }]);
  });

  it("gives a true root no links at all", () => {
    const all = [mk("a", [])];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("a")).toEqual([]);
  });

  it("dedupes two paths reaching the same visible ancestor", () => {
    const all = [
      mk("m", ["h1", "h2"]),
      mk("h1", ["v"], "y"),
      mk("h2", ["v"], "y"),
      mk("v", []),
    ];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("m")).toEqual([{ hash: "v", bridged: true }]);
  });

  it("prefers the direct link when a bridge resolves to the same parent", () => {
    const all = [mk("m", ["v", "h"]), mk("v", []), mk("h", ["v"], "y")];
    const eff = bridgeParents(all, byAuthor("x"));
    expect(eff.get("m")).toEqual([{ hash: "v", bridged: false }]);
  });

  it("caps fan-out at 8, pinning the first-parent link", () => {
    // One child whose 10 hidden parents resolve to 10 distinct visible
    // ancestors. v1 (via the first parent) must survive in slot 0; the
    // rest keep the nearest (earliest-listed) ancestors.
    const hidden = Array.from({ length: 10 }, (_, i) =>
      mk(`h${i + 1}`, [`v${i + 1}`], "y"),
    );
    const visible = Array.from({ length: 10 }, (_, i) => mk(`v${i + 1}`, []));
    const all = [mk("m", hidden.map((h) => h.hash)), ...hidden, ...visible];
    const eff = bridgeParents(all, byAuthor("x"));
    const links = eff.get("m")!;
    expect(links).toHaveLength(8);
    expect(links[0]).toEqual({ hash: "v1", bridged: true });
    expect(links.map((l) => l.hash)).not.toContain("v10");
  });
});
