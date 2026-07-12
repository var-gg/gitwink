import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useLingui } from "@lingui/react/macro";

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
import { filterMessages } from "../i18n/messages/filters";
import { translate } from "../i18n/translate";

// Virtualised-row BASE heights (px) at scale 1.0 — chipRowH scales them
// against the current --ui-scale so JS row heights match the CSS content.
const ITEM_H_BASE = 26; // .chip-item — one-line branch entry
const HEADER_H_BASE = 25; // .chip-section-header — "Local" / "Remote tracking"
const EMPTY_H_BASE = 34; // .chip-empty — "No branches match."

// Mirror of the backend revwalk caps (git.rs) — a returned count above the
// cap is the "more than" sentinel (cap + 1), rendered as e.g. "5,000+".
const LOCAL_COUNT_CAP = 5_000;
const REMOTE_COUNT_CAP = 500;

/** A failed visible-row count is retried a few times because ref walks can
 * briefly race a fetch. The bound is per ref/tip while this dropdown is open:
 * a permanently unreadable ref must never create an IPC retry loop. */
export const BRANCH_COUNT_MAX_RETRIES = 3;
const BRANCH_COUNT_RETRY_BASE_MS = 300;

export function branchCountRetryDelay(retryAttempt: number): number {
  const boundedAttempt = Math.max(
    1,
    Math.min(BRANCH_COUNT_MAX_RETRIES, Math.floor(retryAttempt)),
  );
  return BRANCH_COUNT_RETRY_BASE_MS * 2 ** (boundedAttempt - 1);
}

/** Lazy commit-count label for a branch row: "…" until its count loads,
 * then the number (or "<cap>+" when the walk hit the cap). */
function countLabel(
  b: BranchInfo,
  counts: Map<string, number>,
  failed: Set<string>,
): string {
  const c = counts.get(b.refName);
  if (c == null) return failed.has(b.refName) ? "—" : "…";
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
  const { i18n } = useLingui();
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
  const [failedCounts, setFailedCounts] = useState<Set<string>>(new Set());
  const requestedRef = useRef<Set<string>>(new Set());
  const pendingRef = useRef<Set<string>>(new Set());
  const flushTimerRef = useRef<number | null>(null);
  const retryTimerRef = useRef<number | null>(null);
  const retryQueueRef = useRef<Set<string>>(new Set());
  const retryAttemptsRef = useRef<Map<string, number>>(new Map());
  const visibleRefsRef = useRef<Set<string>>(new Set());
  const requestEpochRef = useRef(0);
  const flushRef = useRef<() => void>(() => {});
  const branchTipsKey = useMemo(
    () =>
      branches
        .map((branch) => `${branch.refName}\u0000${branch.tipHash}`)
        .join("\u0001"),
    [branches],
  );

  // A different repo OR a moved ref makes the old counts invalid. A layout
  // effect advances the epoch in the same commit that exposes the new tips,
  // before a stale promise microtask can publish repo A's identically-named
  // ref into repo B (or an old count into a newly-fetched branch tip).
  useLayoutEffect(() => {
    requestEpochRef.current += 1;
    if (flushTimerRef.current != null) {
      window.clearTimeout(flushTimerRef.current);
      flushTimerRef.current = null;
    }
    if (retryTimerRef.current != null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    setCounts(new Map());
    setFailedCounts(new Set());
    requestedRef.current = new Set();
    pendingRef.current = new Set();
    retryQueueRef.current = new Set();
    retryAttemptsRef.current = new Map();
    visibleRefsRef.current = new Set();
    return () => {
      // Always invalidate in-flight promises on dependency change/unmount.
      // The close-session path may already have advanced this epoch.
      requestEpochRef.current += 1;
      if (flushTimerRef.current != null) {
        window.clearTimeout(flushTimerRef.current);
        flushTimerRef.current = null;
      }
      if (retryTimerRef.current != null) {
        window.clearTimeout(retryTimerRef.current);
        retryTimerRef.current = null;
      }
    };
  }, [repoPath, branchTipsKey]);

  const scheduleRetry = useCallback((refs: string[], requestEpoch: number) => {
    if (requestEpochRef.current !== requestEpoch) return;

    let earliestAttempt = Number.POSITIVE_INFINITY;
    for (const refName of refs) {
      if (!visibleRefsRef.current.has(refName)) {
        // The row left the measured virtual viewport. Release the request key
        // so a later scroll-back can retry it, but keep its attempt budget for
        // this open session.
        requestedRef.current.delete(refName);
        continue;
      }
      const nextAttempt = (retryAttemptsRef.current.get(refName) ?? 0) + 1;
      if (nextAttempt > BRANCH_COUNT_MAX_RETRIES) {
        // Keep it requested after exhaustion: repeated ResizeObserver /
        // virtual-list notifications cannot restart an infinite loop. Closing
        // and reopening the dropdown explicitly grants a fresh bounded budget.
        setFailedCounts((previous) => {
          if (previous.has(refName)) return previous;
          const next = new Set(previous);
          next.add(refName);
          return next;
        });
        continue;
      }
      retryAttemptsRef.current.set(refName, nextAttempt);
      retryQueueRef.current.add(refName);
      // Keep the ref marked requested while it waits. VirtualChipList can
      // legitimately re-report the same mounted keys after any row update;
      // releasing this bit would bypass the backoff with a duplicate 80 ms
      // request (and could overlap the scheduled retry).
      earliestAttempt = Math.min(earliestAttempt, nextAttempt);
    }

    if (retryQueueRef.current.size === 0 || retryTimerRef.current != null) {
      return;
    }
    const delay = branchCountRetryDelay(earliestAttempt);
    retryTimerRef.current = window.setTimeout(() => {
      retryTimerRef.current = null;
      if (requestEpochRef.current !== requestEpoch) return;

      const queued = Array.from(retryQueueRef.current);
      retryQueueRef.current.clear();
      for (const refName of queued) {
        if (!visibleRefsRef.current.has(refName)) {
          requestedRef.current.delete(refName);
          const attempt = retryAttemptsRef.current.get(refName) ?? 0;
          if (attempt <= 1) retryAttemptsRef.current.delete(refName);
          else retryAttemptsRef.current.set(refName, attempt - 1);
          continue;
        }
        requestedRef.current.add(refName);
        pendingRef.current.add(refName);
      }
      if (pendingRef.current.size > 0) flushRef.current();
    }, delay);
  }, []);

  const flush = useCallback(() => {
    flushTimerRef.current = null;
    const refs = Array.from(pendingRef.current);
    pendingRef.current = new Set();
    if (refs.length === 0) return;
    const requestEpoch = requestEpochRef.current;
    void countBranchCommits(repoPath, refs)
      .then((res) => {
        if (requestEpochRef.current !== requestEpoch) return;
        const returned = new Set(res.map(({ refName }) => refName));
        if (res.length > 0) {
          setFailedCounts((previous) => {
            let changed = false;
            const next = new Set(previous);
            for (const { refName } of res) changed = next.delete(refName) || changed;
            return changed ? next : previous;
          });
          setCounts((prev) => {
            const next = new Map(prev);
            for (const { refName, count } of res) {
              retryAttemptsRef.current.delete(refName);
              retryQueueRef.current.delete(refName);
              next.set(refName, count);
            }
            return next;
          });
        }
        // A partial backend response is a failure for the omitted refs. Do
        // not strand those rows at an eternal ellipsis merely because the
        // promise itself resolved.
        const omitted = refs.filter((refName) => !returned.has(refName));
        if (omitted.length > 0) scheduleRetry(omitted, requestEpoch);
      })
      .catch(() => {
        if (requestEpochRef.current !== requestEpoch) return;
        scheduleRetry(refs, requestEpoch);
      });
  }, [repoPath, scheduleRetry]);
  flushRef.current = flush;

  const requestCounts = useCallback(
    (refNames: string[]) => {
      let added = false;
      for (const refName of refNames) {
        if (requestedRef.current.has(refName)) continue;
        if (
          (retryAttemptsRef.current.get(refName) ?? 0) >=
          BRANCH_COUNT_MAX_RETRIES
        ) {
          // The automatic budget is exhausted until the user closes/reopens
          // the dropdown (or the repo/ref tip changes and advances epoch).
          requestedRef.current.add(refName);
          continue;
        }
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
      if (retryTimerRef.current != null) window.clearTimeout(retryTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    if (open) return;
    // Closing is cancellation, not merely hiding: a slow count from the old
    // session must not publish into a later reopen of the same repo/ref tip.
    requestEpochRef.current += 1;
    visibleRefsRef.current.clear();
    retryQueueRef.current.clear();
    retryAttemptsRef.current.clear();
    setFailedCounts(new Set());
    for (const refName of pendingRef.current) requestedRef.current.delete(refName);
    pendingRef.current.clear();
    if (flushTimerRef.current != null) {
      window.clearTimeout(flushTimerRef.current);
      flushTimerRef.current = null;
    }
    if (retryTimerRef.current != null) {
      window.clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    // Loaded counts stay warm. Only unresolved refs are released so reopening
    // is an explicit, user-bounded retry instead of a permanent dead end.
    for (const refName of requestedRef.current) {
      if (!counts.has(refName)) requestedRef.current.delete(refName);
    }
  }, [open, counts]);

  // VirtualChipList reports the keys it has mounted; pull branch refs out and
  // queue their counts. Header/"all" rows are ignored.
  const onVisibleKeys = useCallback(
    (keys: string[]) => {
      const refs: string[] = [];
      for (const k of keys) {
        if (k.startsWith("branch:")) refs.push(k.slice("branch:".length));
      }
      const nextVisible = new Set(refs);
      visibleRefsRef.current = nextVisible;
      for (const refName of retryQueueRef.current) {
        if (nextVisible.has(refName)) continue;
        retryQueueRef.current.delete(refName);
        requestedRef.current.delete(refName);
        const attempt = retryAttemptsRef.current.get(refName) ?? 0;
        if (attempt <= 1) retryAttemptsRef.current.delete(refName);
        else retryAttemptsRef.current.set(refName, attempt - 1);
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
    if (loading && branches.length === 0) {
      return i18n._(filterMessages.loadingBranches);
    }
    if (selected === "all") {
      return translate(i18n, filterMessages.allBranchesCount, {
        count: branches.length,
      });
    }
    if (selected.length === 0) return i18n._(filterMessages.noBranches);
    if (selected.length === 1) {
      const only = branches.find((b) => b.refName === selected[0]);
      return (
        only?.name ??
        translate(i18n, filterMessages.branchUnavailable, {
          branch: displayRefName(selected[0]),
        })
      );
    }
    return unavailableSelected.length > 0
      ? translate(i18n, filterMessages.branchCountUnavailable, {
          count: selected.length,
          unavailable: unavailableSelected.length,
        })
      : translate(i18n, filterMessages.branchCount, {
          count: selected.length,
        });
  }, [selected, branches, unavailableSelected, loading, i18n, i18n.locale]);

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
            role="checkbox"
            aria-checked={isSelected}
            title={b.refName}
            onClick={() => toggle(b.refName)}
          >
            <span className="chip-check" aria-hidden="true">
              {isSelected ? "✓" : ""}
            </span>
            <span className="chip-item-name" title={b.name}>
              {b.name}
              {b.isHead && <span className="chip-item-head"> · HEAD</span>}
            </span>
            <span className="chip-branch-tail">
              <span
                className="chip-branch-age"
                title={translate(i18n, filterMessages.lastActivity, {
                  date: fullDateTime(b.lastActivity, i18n.locale),
                })}
              >
                {compactAge(b.lastActivity)}
              </span>
              <span
                className="chip-branch-count"
                title={
                  failedCounts.has(b.refName)
                    ? translate(i18n, filterMessages.countUnavailable, {
                        count: BRANCH_COUNT_MAX_RETRIES,
                      })
                    : i18n._(filterMessages.reachableCommits)
                }
              >
                {countLabel(b, counts, failedCounts)}
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
          role="checkbox"
          aria-checked={selected === "all"}
          onClick={() => {
            onChange("all");
            onClose();
          }}
        >
          <span className="chip-item-name">
            {i18n._(filterMessages.allBranches)}
          </span>
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
            title={i18n._(filterMessages.unavailableRefsTitle)}
          >
            {i18n._(filterMessages.unavailableRefs)}
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
              role="checkbox"
              aria-checked={true}
              title={translate(i18n, filterMessages.unavailableRefRemove, {
                ref: refName,
              })}
              onClick={() => {
                onChange(removeUnavailableFacetValue(selected, refName));
              }}
            >
              <span className="chip-check" aria-hidden="true">
                ✓
              </span>
              <span
                className="chip-item-name"
                title={displayRefName(refName)}
              >
                {displayRefName(refName)}
              </span>
              <span className="chip-item-count">
                {i18n._(filterMessages.missingRef)}
              </span>
            </button>
          ),
        });
      }
    }
    if (localBranches.length > 0) {
      out.push({
        key: "__local",
        height: HEADER_H,
        render: () => (
          <div className="chip-section-header">
            {i18n._(filterMessages.localBranches)}
          </div>
        ),
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
            title={i18n._(filterMessages.remoteTrackingTitle)}
          >
            {i18n._(filterMessages.remoteTracking)}
          </div>
        ),
      });
      for (const b of remoteBranches) out.push(branchRow(b));
    }
    if (loading && localBranches.length === 0 && remoteBranches.length === 0) {
      out.push({
        key: "__loading",
        height: EMPTY_H,
        render: () => (
          <div className="chip-empty">
            {i18n._(filterMessages.loadingBranches)}
          </div>
        ),
      });
    } else if (
      localBranches.length === 0 &&
      remoteBranches.length === 0 &&
      visibleUnavailable.length === 0
    ) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => (
          <div className="chip-empty">
            {i18n._(filterMessages.noBranchesMatch)}
          </div>
        ),
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
    failedCounts,
    i18n,
    i18n.locale,
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
          placeholder={i18n._(filterMessages.searchBranches)}
          aria-label={i18n._(filterMessages.searchBranchesAria)}
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
