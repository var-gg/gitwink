import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { countBranchCommits } from "../lib/ipc";
import {
  removeUnavailableFacetValue,
  toggleFacetValue,
  unavailableFacetValues,
} from "../lib/facetSelection";
import { chipRowH, useUiScale } from "../lib/settings";
import { compactAge, fullDateTime } from "../lib/time";
import type { BranchInfo } from "../types";
import { ChipDropdown } from "./ChipDropdown";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";

// Virtualised-row BASE heights (px) at scale 1.0 — chipRowH scales them
// against the current --ui-scale so JS row heights match the CSS content.
const ITEM_H_BASE = 26; // .chip-item — one-line branch entry
const HEADER_H_BASE = 25; // .chip-section-header — "Local" / "Remote tracking"
const EMPTY_H_BASE = 34; // .chip-empty — "No branches match."

// Mirror of the backend revwalk caps (git.rs) — a returned count above the
// cap is the "more than" sentinel (cap + 1), rendered as e.g. "5,000+".
const LOCAL_COUNT_CAP = 5_000;
const REMOTE_COUNT_CAP = 500;

/** Lazy commit-count label for a branch row: "…" until its count loads,
 * then the number (or "<cap>+" when the walk hit the cap). */
function countLabel(b: BranchInfo, counts: Map<string, number>): string {
  const c = counts.get(b.refName);
  if (c == null) return "…";
  const cap = b.kind === "remote" ? REMOTE_COUNT_CAP : LOCAL_COUNT_CAP;
  return c > cap ? `${cap.toLocaleString()}+` : c.toLocaleString();
}

function displayRefName(refName: string): string {
  return refName
    .replace(/^refs\/heads\//, "")
    .replace(/^refs\/remotes\//, "");
}

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  /** Repo the branches belong to — needed to lazily count commits per ref. */
  repoPath: string;
  branches: BranchInfo[];
  /** True while the current repo's ref list is loading. Prevents a saved
   * selection from being mislabeled unavailable during the async handoff. */
  loading: boolean;
  /** Array of refNames (e.g. "refs/heads/main", "refs/remotes/origin/main")
   * or the "all" sentinel. We key by refName so a local "main" and a
   * remote "origin/main" never collide. */
  selected: string[] | "all";
  onChange: (sel: string[] | "all") => void;
}

export function BranchChip({
  open,
  onToggle,
  onClose,
  repoPath,
  branches,
  loading,
  selected,
  onChange,
}: Props) {
  const [query, setQuery] = useState("");
  const scale = useUiScale();

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  // ----- lazy per-branch commit counts -----
  // The branch list loads instantly with no counts (counting is a per-branch
  // history walk). We fetch counts only for the rows VirtualChipList reports
  // as on-screen, deduped + debounced into one IPC call per batch.
  const [counts, setCounts] = useState<Map<string, number>>(new Map());
  const requestedRef = useRef<Set<string>>(new Set());
  const pendingRef = useRef<Set<string>>(new Set());
  const flushTimerRef = useRef<number | null>(null);

  // A different repo's refs/counts no longer apply — start clean.
  useEffect(() => {
    setCounts(new Map());
    requestedRef.current = new Set();
    pendingRef.current = new Set();
  }, [repoPath]);

  const flush = useCallback(() => {
    flushTimerRef.current = null;
    const refs = Array.from(pendingRef.current);
    pendingRef.current = new Set();
    if (refs.length === 0) return;
    void countBranchCommits(repoPath, refs)
      .then((res) => {
        if (res.length === 0) return;
        setCounts((prev) => {
          const next = new Map(prev);
          for (const { refName, count } of res) next.set(refName, count);
          return next;
        });
      })
      .catch(() => {
        // Let a later visibility pass retry these refs.
        for (const r of refs) requestedRef.current.delete(r);
      });
  }, [repoPath]);

  const requestCounts = useCallback(
    (refNames: string[]) => {
      let added = false;
      for (const refName of refNames) {
        if (requestedRef.current.has(refName)) continue;
        requestedRef.current.add(refName);
        pendingRef.current.add(refName);
        added = true;
      }
      if (added && flushTimerRef.current == null) {
        flushTimerRef.current = window.setTimeout(flush, 80);
      }
    },
    [flush],
  );

  useEffect(
    () => () => {
      if (flushTimerRef.current != null) window.clearTimeout(flushTimerRef.current);
    },
    [],
  );

  // VirtualChipList reports the keys it has mounted; pull branch refs out and
  // queue their counts. Header/"all" rows are ignored.
  const onVisibleKeys = useCallback(
    (keys: string[]) => {
      const refs: string[] = [];
      for (const k of keys) {
        if (k.startsWith("branch:")) refs.push(k.slice("branch:".length));
      }
      if (refs.length > 0) requestCounts(refs);
    },
    [requestCounts],
  );

  // Snapshot of `selected` taken when the dropdown opens. The list order
  // is frozen against this — toggling a ✓ while the dropdown is open
  // never makes a row jump under the cursor. Reopening re-snapshots, so
  // the just-checked branches float to the top then (VS Code's pattern).
  const [snapshot, setSnapshot] = useState<string[] | "all">(selected);
  useEffect(() => {
    if (open) setSnapshot(selected);
    // `selected` is intentionally omitted: the snapshot must NOT update
    // while the dropdown stays open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const { localBranches, remoteBranches } = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = (b: BranchInfo) => !q || b.name.toLowerCase().includes(q);
    const snapSet = snapshot === "all" ? null : new Set(snapshot);
    // Branches selected at open-time sort to the top of their section.
    // The sort key is the snapshot, so the order holds steady while open.
    const rank = (b: BranchInfo) => (snapSet?.has(b.refName) ? 0 : 1);
    const bySnapshot = (a: BranchInfo, b: BranchInfo) => rank(a) - rank(b);
    return {
      localBranches: branches
        .filter((b) => b.kind === "local" && match(b))
        .sort(bySnapshot),
      remoteBranches: branches
        .filter((b) => b.kind === "remote" && match(b))
        .sort(bySnapshot),
    };
  }, [branches, query, snapshot]);

  const branchRefNames = useMemo(
    () => branches.map((branch) => branch.refName),
    [branches],
  );
  const unavailableSelected = useMemo(
    () => (loading ? [] : unavailableFacetValues(branchRefNames, selected)),
    [branchRefNames, selected, loading],
  );
  const visibleUnavailable = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q
      ? unavailableSelected.filter((refName) =>
          displayRefName(refName).toLowerCase().includes(q),
        )
      : unavailableSelected;
  }, [query, unavailableSelected]);

  // Label adapts to the count. For a single selection we show the branch
  // name itself — minimum cognitive load — and fall back to a count when
  // multiple are picked.
  const label = useMemo(() => {
    if (loading && branches.length === 0) return "Loading branches…";
    if (selected === "all") return `All branches (${branches.length})`;
    if (selected.length === 0) return "No branches";
    if (selected.length === 1) {
      const only = branches.find((b) => b.refName === selected[0]);
      return only?.name ?? `${displayRefName(selected[0])} · unavailable`;
    }
    return unavailableSelected.length > 0
      ? `${selected.length} branches · ${unavailableSelected.length} unavailable`
      : `${selected.length} branches`;
  }, [selected, branches, unavailableSelected, loading]);

  const toggle = useCallback(
    (refName: string) => {
      onChange(toggleFacetValue(selected, refName, branchRefNames));
    },
    [selected, branchRefNames, onChange],
  );

  // Flatten the two sections into one virtual-row list. The Local / Remote
  // headers and the empty-state line are known-height special rows
  // interleaved with the branch rows.
  const rows = useMemo<VirtualChipRow[]>(() => {
    const ITEM_H = chipRowH(scale, ITEM_H_BASE);
    const HEADER_H = chipRowH(scale, HEADER_H_BASE);
    const EMPTY_H = chipRowH(scale, EMPTY_H_BASE);

    const branchRow = (b: BranchInfo): VirtualChipRow => {
      // In the "all" meta-state the All branches row at the top carries the
      // highlight — individual items shouldn't ALSO look "checked", or a
      // user clicking a row to "uncheck it" is met with the GitLens focus
      // behaviour and it feels like a different row got deselected. So: no
      // per-item ✓ until the user makes an explicit selection.
      const isSelected =
        selected !== "all" && (selected as string[]).includes(b.refName);
      return {
        key: "branch:" + b.refName,
        height: ITEM_H,
        render: () => (
          <button
            type="button"
            className={"chip-item" + (isSelected ? " checked" : "")}
            onClick={() => toggle(b.refName)}
          >
            <span className="chip-check">{isSelected ? "✓" : ""}</span>
            <span className="chip-item-name">
              {b.name}
              {b.isHead && <span className="chip-item-head"> · HEAD</span>}
            </span>
            <span className="chip-branch-tail">
              <span
                className="chip-branch-age"
                title={`Last activity: ${fullDateTime(b.lastActivity)}`}
              >
                {compactAge(b.lastActivity)}
              </span>
              <span
                className="chip-branch-count"
                title="Reachable commits across all time (independent of the time filter)"
              >
                {countLabel(b, counts)}
              </span>
            </span>
          </button>
        ),
      };
    };

    const out: VirtualChipRow[] = [];
    out.push({
      key: "__all",
      height: ITEM_H,
      render: () => (
        <button
          type="button"
          className={"chip-item" + (selected === "all" ? " active" : "")}
          onClick={() => {
            onChange("all");
            onClose();
          }}
        >
          <span className="chip-item-name">All branches</span>
        </button>
      ),
    });
    if (visibleUnavailable.length > 0) {
      out.push({
        key: "__unavailable",
        height: HEADER_H,
        render: () => (
          <div
            className="chip-section-header chip-section-warning"
            title="Selected refs that no longer exist in this repository"
          >
            Selected · unavailable refs
          </div>
        ),
      });
      for (const refName of visibleUnavailable) {
        out.push({
          key: "unavailable-branch:" + refName,
          height: ITEM_H,
          render: () => (
            <button
              type="button"
              className="chip-item checked chip-item-unavailable"
              title={`${refName} no longer exists. Click to remove this filter.`}
              onClick={() => {
                onChange(removeUnavailableFacetValue(selected, refName));
              }}
            >
              <span className="chip-check">✓</span>
              <span className="chip-item-name">{displayRefName(refName)}</span>
              <span className="chip-item-count">missing</span>
            </button>
          ),
        });
      }
    }
    if (localBranches.length > 0) {
      out.push({
        key: "__local",
        height: HEADER_H,
        render: () => <div className="chip-section-header">Local</div>,
      });
      for (const b of localBranches) out.push(branchRow(b));
    }
    if (remoteBranches.length > 0) {
      out.push({
        key: "__remote",
        height: HEADER_H,
        render: () => (
          <div
            className="chip-section-header"
            title="Remote-tracking refs are read locally — refreshed by your IDE / CLI, or by gitwink's auto-fetch on panel open (Settings → Auto-fetch)."
          >
            Remote tracking
          </div>
        ),
      });
      for (const b of remoteBranches) out.push(branchRow(b));
    }
    if (loading && localBranches.length === 0 && remoteBranches.length === 0) {
      out.push({
        key: "__loading",
        height: EMPTY_H,
        render: () => <div className="chip-empty">Loading branches…</div>,
      });
    } else if (
      localBranches.length === 0 &&
      remoteBranches.length === 0 &&
      visibleUnavailable.length === 0
    ) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => <div className="chip-empty">No branches match.</div>,
      });
    }
    return out;
  }, [
    localBranches,
    remoteBranches,
    visibleUnavailable,
    loading,
    selected,
    onChange,
    onClose,
    toggle,
    scale,
    counts,
  ]);

  return (
    <ChipDropdown
      id="branch"
      label={label}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
      active={selected !== "all"}
    >
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search branches…"
        />
      </div>
      <VirtualChipList
        rows={rows}
        resetKey={query}
        onVisibleKeys={onVisibleKeys}
      />
    </ChipDropdown>
  );
}
