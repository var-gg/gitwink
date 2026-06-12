// Parent bridging for a filtered DAG view.
//
// The single-repo lane graph can be narrowed by an author filter. The
// surviving rows are NOT ancestry-closed: a visible commit's parent is
// usually someone else's (hidden) commit. Feeding those raw parent links
// to the lane allocator leaks one lane per missing parent — the lane sits
// open forever waiting for a commit that never appears — so the graph
// degrades into a staircase of dots over a bundle of full-height lines.
//
// `bridgeParents` rewrites each visible commit's parents to its NEAREST
// VISIBLE ANCESTORS: hidden commits are walked through (memoized over the
// full loaded list), and every link that crossed hidden commits is flagged
// `bridged` so the drawer can render it dashed ("history elided here").
// A chain that leaves the loaded window without reaching a visible commit
// is dropped — except when the commit would otherwise have no downward
// link at all, where a single off-window marker (`hash: null`) keeps the
// "continues below" tail line.

import type { CommitSummary } from "../types";

/** One rewritten parent link of a visible commit. */
export interface EffectiveParent {
  /** Nearest visible ancestor, or null = the chain leaves the loaded
   *  window (drawn as a line running off the bottom). */
  hash: string | null;
  /** True when hidden commits sit between child and ancestor. */
  bridged: boolean;
}

/** A merge fan-in through many hidden commits can resolve to a wide set of
 *  visible ancestors. Beyond this, far links add lane width without adding
 *  orientation value — keep the trunk link plus the nearest few. */
const MAX_EFFECTIVE_PARENTS = 8;

/** What a hash resolves to once hidden commits are walked through. */
interface Resolved {
  /** Visible ancestor hashes, deduped, in discovery (parent) order. */
  visibles: string[];
  /** Some path exited the loaded list before reaching a visible commit. */
  off: boolean;
}

/**
 * Rewrite parent links of the visible subset of `all` (newest-first) so
 * they point at nearest visible ancestors. Returns a map keyed by visible
 * commit hash; every visible commit has an entry.
 */
export function bridgeParents(
  all: CommitSummary[],
  isVisible: (c: CommitSummary) => boolean,
): Map<string, EffectiveParent[]> {
  const byHash = new Map<string, CommitSummary>();
  const indexOf = new Map<string, number>();
  all.forEach((c, i) => {
    byHash.set(c.hash, c);
    indexOf.set(c.hash, i);
  });

  // Memoized resolution — the DAG is acyclic, so plain recursion with a
  // memo is linear in edges. Depth is bounded by the longest hidden chain
  // (≤ the loaded list, capped server-side), safely within stack limits.
  const memo = new Map<string, Resolved>();
  function resolve(hash: string): Resolved {
    const hit = memo.get(hash);
    if (hit) return hit;
    const c = byHash.get(hash);
    let res: Resolved;
    if (!c) {
      res = { visibles: [], off: true };
    } else if (isVisible(c)) {
      res = { visibles: [hash], off: false };
    } else {
      const seen = new Set<string>();
      const visibles: string[] = [];
      let off = false;
      for (const p of c.parents) {
        const r = resolve(p);
        for (const v of r.visibles) {
          if (!seen.has(v)) {
            seen.add(v);
            visibles.push(v);
          }
        }
        off = off || r.off;
      }
      res = { visibles, off };
    }
    memo.set(hash, res);
    return res;
  }

  const out = new Map<string, EffectiveParent[]>();
  for (const c of all) {
    if (!isVisible(c)) continue;

    const seen = new Set<string>();
    const eff: EffectiveParent[] = [];
    // Track HOW a chain exits the window: a direct off-window parent is
    // the ordinary loaded-window tail (drawn solid, like the unfiltered
    // view); an exit through hidden commits is elided history (dashed).
    let offDirect = false;
    let offBridged = false;
    for (const p of c.parents) {
      const parent = byHash.get(p);
      if (parent && isVisible(parent)) {
        if (!seen.has(p)) {
          seen.add(p);
          eff.push({ hash: p, bridged: false });
        }
        continue;
      }
      if (!parent) {
        offDirect = true;
        continue;
      }
      const r = resolve(p);
      for (const v of r.visibles) {
        if (!seen.has(v)) {
          seen.add(v);
          eff.push({ hash: v, bridged: true });
        }
      }
      offBridged = offBridged || r.off;
    }

    let trimmed = eff;
    if (eff.length > MAX_EFFECTIVE_PARENTS) {
      // Keep the first-parent link in place (it carries the trunk lane),
      // then prefer the nearest ancestors among the rest.
      const [head, ...rest] = eff;
      rest.sort(
        (a, b) =>
          (indexOf.get(a.hash as string) ?? 0) -
          (indexOf.get(b.hash as string) ?? 0),
      );
      trimmed = [head, ...rest.slice(0, MAX_EFFECTIVE_PARENTS - 1)];
    }
    // No visible ancestor at all, but ancestry continues below the loaded
    // window — keep one tail line so the commit doesn't read as a root.
    if (trimmed.length === 0 && (offDirect || offBridged)) {
      trimmed = [{ hash: null, bridged: !offDirect }];
    }
    out.set(c.hash, trimmed);
  }
  return out;
}
