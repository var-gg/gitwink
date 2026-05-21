// Windowed-pull data layer for the all-repos timeline.
//
// The all-repos timeline can span an unbounded number of commits, so the
// frontend never holds the full set. This hook keeps ONE contiguous window
// loaded — tracked by `baseIndex`, the global rank of `rows[0]` — inside a
// `count`-tall virtual scroll space. The window is:
//   • extended a keyset page at a time as the user wheel-scrolls past an
//     edge (`extendOlder` / `extendNewer`),
//   • replaced wholesale when the user drags the scrollbar far — a
//     debounced jump via `list_commits_at_rank`, and
//   • re-anchored around the previously-focused commit on a filter change
//     (`list_commits_around_anchor`), so the viewport recovers instead of
//     snapping back to the top.
// A pinned `viewGeneration` keeps the scanner's concurrent inserts from
// disturbing the page sequence; a monotonic query id drops stale responses.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { CommitSummary, Cursor, TimelineFilters } from "../types";
import {
  countCommits,
  getTimelineGeneration,
  listCommitsAroundAnchor,
  listCommitsAtRank,
  listCommitsWindow,
  recentCommits,
} from "./ipc";

/** Rows fetched per keyset extend page. */
const PAGE_SIZE = 60;
/** A jump / recovery loads this many rows newer + older of the anchor — a
 *  cushion so scrolling around the landing spot stays smooth. */
const ANCHOR_BEFORE = 60;
const ANCHOR_AFTER = 150;
/** Viewport-to-window gap (rows) at or under which an extend beats a jump. */
const EXTEND_GAP = 120;
/** Collapse a flurry of scroll events (a scrollbar drag) into one jump. */
const JUMP_DEBOUNCE_MS = 100;
/** Hard cap on the loaded window. Extending past it evicts the far edge,
 *  so an uninterrupted scroll can't accumulate the whole history in
 *  memory — the window stays bounded regardless of scroll distance. */
const MAX_WINDOW = 600;

/** Stable identity for a commit across reloads. */
function commitKey(c: { repoPath: string; hash: string }): string {
  return `${c.repoPath}:${c.hash}`;
}
function nowSec(): number {
  return Math.floor(Date.now() / 1000);
}

/** A commit's keyset cursor, reconstructed from the row itself — the cursor
 *  is (sort_ts, repo_path, hash) with sort_ts = -timestamp, all carried by
 *  the CommitSummary. Used to recompute an edge cursor after a window trim. */
function cursorOf(c: CommitSummary): Cursor {
  return { sortTs: -c.timestamp, repoPath: c.repoPath, hash: c.hash };
}

export interface TimelineWindowParams {
  /** repo-id filter, or null for all repos */
  repoIds: number[] | null;
  /** author-name filter, or null for all authors */
  authors: string[] | null;
  /** time window in days, or null for all time */
  windowDays: number | null;
  /** bumped by the caller to force a reload (panel re-summoned) */
  refreshNonce: number;
}

export interface TimelineWindowState {
  /** the loaded window — a contiguous slice of the filtered timeline */
  rows: CommitSummary[];
  /** global rank of `rows[0]` in the filtered total order */
  baseIndex: number;
  /** total filtered commits (pinned snapshot) — the scroll track size */
  count: number;
  status: "loading" | "ready" | "error";
  /** `repoPath:hash` keys of commits that arrived during a live reload —
   *  rendered with the "new" marker. */
  freshHashes: Set<string>;
  /** scroll-to signal — when `nonce` changes, the timeline scrolls global
   *  row `index` to the viewport top (initial load / filter recovery /
   *  reload-to-latest). */
  recovery: { index: number; nonce: number };
  /** the timeline reports its viewport's global row span whenever it
   *  changes; the hook extends or jump-loads the window to cover it. */
  requestRange: (firstGlobal: number, lastGlobal: number) => void;
  /** re-pin the generation and reload from the newest commit — the "N new"
   *  pill and at-top invalidation; diffs freshly-arrived commits. */
  reloadLatest: () => void;
  /** commits beyond the pinned snapshot — the "N new" pill count. */
  countNew: () => Promise<number>;
}

/** The loaded window, mirrored out of React state so the async machinery
 *  reads live values without being re-created per render. */
interface WindowRef {
  rows: CommitSummary[];
  baseIndex: number;
  startCursor: Cursor | null;
  endCursor: Cursor | null;
  filter: TimelineFilters | null;
  count: number;
}

export function useTimelineWindow(
  params: TimelineWindowParams,
): TimelineWindowState {
  const { repoIds, authors, windowDays, refreshNonce } = params;

  const [rows, setRows] = useState<CommitSummary[]>([]);
  const [baseIndex, setBaseIndex] = useState(0);
  const [count, setCount] = useState(0);
  const [status, setStatus] = useState<"loading" | "ready" | "error">(
    "loading",
  );
  const [freshHashes, setFreshHashes] = useState<Set<string>>(() => new Set());
  const [recovery, setRecovery] = useState({ index: 0, nonce: 0 });

  const windowRef = useRef<WindowRef>({
    rows: [],
    baseIndex: 0,
    startCursor: null,
    endCursor: null,
    filter: null,
    count: 0,
  });
  // Monotonic query id — every reload / jump bumps it; an in-flight
  // response whose id is stale is dropped.
  const queryRef = useRef(0);
  // qid of an in-flight full reload or jump, or 0 — `reconcile` stands
  // down while one of those is replacing the window out from under it.
  const loadQidRef = useRef(0);
  const paramsRef = useRef(params);
  paramsRef.current = params;
  // Latest viewport span the timeline asked to see (global row indices).
  const lastRangeRef = useRef<{ first: number; last: number }>({
    first: 0,
    last: 0,
  });
  const loadingNewerRef = useRef(false);
  const loadingOlderRef = useRef(false);
  const jumpTimerRef = useRef<number | null>(null);
  const jumpTargetRef = useRef(0);

  // All the async windowing machinery. Built once — every function reads
  // refs + the stable state setters, never render-scoped values, so a
  // single construction stays correct for the hook's lifetime.
  const machinery = useMemo(() => {
    function cancelPendingJump() {
      if (jumpTimerRef.current != null) {
        window.clearTimeout(jumpTimerRef.current);
        jumpTimerRef.current = null;
      }
    }

    function scheduleJump(rank: number) {
      jumpTargetRef.current = rank;
      if (jumpTimerRef.current != null) {
        window.clearTimeout(jumpTimerRef.current);
      }
      jumpTimerRef.current = window.setTimeout(() => {
        jumpTimerRef.current = null;
        void doJump(jumpTargetRef.current);
      }, JUMP_DEBOUNCE_MS);
    }

    /** Decide how to cover the latest requested viewport range: it is
     *  already loaded (no-op), one keyset page or two away (extend), or
     *  far (debounced jump). Re-runs after each extend, to chase. */
    function reconcile() {
      // A reload / jump is replacing the window — leave coverage to it.
      if (loadQidRef.current !== 0) return;
      const w = windowRef.current;
      if (!w.filter) return;
      const cnt = w.count;
      if (cnt <= 0) {
        cancelPendingJump();
        return;
      }
      const { first, last } = lastRangeRef.current;
      const winTop = w.baseIndex;
      const winBot = w.baseIndex + w.rows.length;
      const needFirst = Math.max(0, Math.min(first, cnt - 1));
      const needLast = Math.max(needFirst + 1, Math.min(last, cnt));

      if (needFirst >= winTop && needLast <= winBot) {
        cancelPendingJump();
        return;
      }
      if (needFirst < winTop) {
        if (winTop - needFirst <= EXTEND_GAP) {
          cancelPendingJump();
          extendNewer();
        } else {
          scheduleJump(needFirst);
        }
        return;
      }
      // needLast > winBot — the viewport runs off the bottom of the window.
      if (needLast - winBot <= EXTEND_GAP && winBot < cnt) {
        cancelPendingJump();
        extendOlder();
      } else {
        scheduleJump(needFirst);
      }
    }

    function extendOlder() {
      const w = windowRef.current;
      if (loadingOlderRef.current || !w.filter || !w.endCursor) return;
      if (w.baseIndex + w.rows.length >= w.count) return;
      loadingOlderRef.current = true;
      const qid = queryRef.current;
      const { filter, endCursor } = w;
      listCommitsWindow(filter, endCursor, "older", PAGE_SIZE)
        .then((win) => {
          if (qid !== queryRef.current) return;
          const cur = windowRef.current;
          let rows = [...cur.rows, ...win.rows];
          let baseIndex = cur.baseIndex;
          let startCursor = cur.startCursor;
          // Cap the window — evict the rows now far above the viewport.
          if (rows.length > MAX_WINDOW) {
            const trim = rows.length - MAX_WINDOW;
            rows = rows.slice(trim);
            baseIndex += trim;
            startCursor = cursorOf(rows[0]);
          }
          windowRef.current = {
            ...cur,
            rows,
            baseIndex,
            startCursor,
            endCursor: win.endCursor ?? cur.endCursor,
          };
          setRows(rows);
          setBaseIndex(baseIndex);
        })
        .catch(() => {})
        .finally(() => {
          loadingOlderRef.current = false;
          if (qid === queryRef.current) reconcile();
        });
    }

    function extendNewer() {
      const w = windowRef.current;
      if (loadingNewerRef.current || !w.filter || !w.startCursor) return;
      if (w.baseIndex <= 0) return;
      loadingNewerRef.current = true;
      const qid = queryRef.current;
      const { filter, startCursor } = w;
      listCommitsWindow(filter, startCursor, "newer", PAGE_SIZE)
        .then((win) => {
          if (qid !== queryRef.current) return;
          const cur = windowRef.current;
          let rows = [...win.rows, ...cur.rows];
          const baseIndex = Math.max(0, cur.baseIndex - win.rows.length);
          let endCursor = cur.endCursor;
          // Cap the window — evict the rows now far below the viewport.
          if (rows.length > MAX_WINDOW) {
            rows = rows.slice(0, MAX_WINDOW);
            endCursor = cursorOf(rows[rows.length - 1]);
          }
          windowRef.current = {
            ...cur,
            rows,
            baseIndex,
            startCursor: win.startCursor ?? cur.startCursor,
            endCursor,
          };
          setRows(rows);
          setBaseIndex(baseIndex);
        })
        .catch(() => {})
        .finally(() => {
          loadingNewerRef.current = false;
          if (qid === queryRef.current) reconcile();
        });
    }

    /** Random-access jump: discard the window, load a fresh one centred on
     *  `rank`. Supersedes any in-flight extend; `reconcile` stands down
     *  until it lands, so no extend races against the swap. */
    async function doJump(rank: number) {
      const w = windowRef.current;
      if (!w.filter) return;
      const qid = ++queryRef.current;
      loadQidRef.current = qid;
      loadingNewerRef.current = false;
      loadingOlderRef.current = false;
      try {
        const around = await listCommitsAtRank(
          w.filter,
          rank,
          ANCHOR_BEFORE,
          ANCHOR_AFTER,
        );
        if (qid !== queryRef.current) return;
        windowRef.current = {
          ...windowRef.current,
          rows: around.rows,
          baseIndex: around.baseIndex,
          startCursor: around.startCursor,
          endCursor: around.endCursor,
        };
        setRows(around.rows);
        setBaseIndex(around.baseIndex);
      } catch {
        /* leave the window in place; a later scroll retries */
      } finally {
        if (loadQidRef.current === qid) loadQidRef.current = 0;
      }
      reconcile();
    }

    /** Full (re)load: re-pin the generation, refetch the count, and load a
     *  window — from the top, around the previously-focused commit, or at
     *  the current viewport rank (a quiet in-place re-pull). */
    async function reload(
      anchorMode: "top" | "recover" | "current",
      soft: boolean,
      emitRecovery: boolean,
    ) {
      const p = paramsRef.current;
      const qid = ++queryRef.current;
      loadQidRef.current = qid;
      loadingNewerRef.current = false;
      loadingOlderRef.current = false;
      cancelPendingJump();
      setStatus("loading");

      // Capture the focus anchor from the OLD window before it is replaced.
      const currentRank = Math.max(0, lastRangeRef.current.first);
      let anchorCursor: Cursor | null = null;
      let focusedHash: string | null = null;
      if (anchorMode === "recover") {
        const w = windowRef.current;
        const li = currentRank - w.baseIndex;
        if (li >= 0 && li < w.rows.length) {
          const c = w.rows[li];
          focusedHash = c.hash;
          anchorCursor = {
            sortTs: -c.timestamp,
            repoPath: c.repoPath,
            hash: c.hash,
          };
        }
      }
      const priorKeys = soft
        ? new Set(windowRef.current.rows.map(commitKey))
        : null;

      try {
        const generation = await getTimelineGeneration();
        if (qid !== queryRef.current) return;
        const since =
          p.windowDays == null ? null : nowSec() - p.windowDays * 86_400;
        const filter: TimelineFilters = {
          repoIds: p.repoIds,
          authors: p.authors,
          since,
          viewGeneration: generation,
        };
        const cnt = await countCommits(filter);
        if (qid !== queryRef.current) return;

        let resRows: CommitSummary[];
        let resBase: number;
        let resStart: Cursor | null;
        let resEnd: Cursor | null;
        if (anchorMode === "recover" && anchorCursor) {
          const a = await listCommitsAroundAnchor(
            filter,
            anchorCursor,
            ANCHOR_BEFORE,
            ANCHOR_AFTER,
          );
          resRows = a.rows;
          resBase = a.baseIndex;
          resStart = a.startCursor;
          resEnd = a.endCursor;
        } else if (anchorMode === "current") {
          const a = await listCommitsAtRank(
            filter,
            currentRank,
            ANCHOR_BEFORE,
            ANCHOR_AFTER,
          );
          resRows = a.rows;
          resBase = a.baseIndex;
          resStart = a.startCursor;
          resEnd = a.endCursor;
        } else {
          const win = await listCommitsWindow(filter, null, "older", PAGE_SIZE);
          resRows = win.rows;
          resBase = 0;
          resStart = win.startCursor;
          resEnd = win.endCursor;
        }
        if (qid !== queryRef.current) return;

        windowRef.current = {
          rows: resRows,
          baseIndex: resBase,
          startCursor: resStart,
          endCursor: resEnd,
          filter,
          count: cnt,
        };
        setRows(resRows);
        setBaseIndex(resBase);
        setCount(cnt);
        setStatus("ready");

        if (emitRecovery) {
          let idx = 0;
          if (anchorMode === "recover" && anchorCursor) {
            const li = focusedHash
              ? resRows.findIndex((r) => r.hash === focusedHash)
              : -1;
            // Anchor survived the filter → land on it; gone → land where it
            // would have been (the newer half is `ANCHOR_BEFORE` rows).
            idx =
              li >= 0
                ? resBase + li
                : resBase + Math.min(ANCHOR_BEFORE, resRows.length);
          }
          idx = Math.max(0, Math.min(idx, Math.max(0, cnt - 1)));
          setRecovery((r) => ({ index: idx, nonce: r.nonce + 1 }));
        }

        if (priorKeys) {
          const added = resRows
            .map(commitKey)
            .filter((k) => !priorKeys.has(k));
          if (added.length > 0) {
            setFreshHashes((prev) => {
              const next = new Set(prev);
              for (const k of added) next.add(k);
              return next;
            });
          }
        } else {
          setFreshHashes(new Set());
        }
      } catch {
        if (qid === queryRef.current) setStatus("error");
      } finally {
        if (loadQidRef.current === qid) loadQidRef.current = 0;
      }
    }

    async function countNew(): Promise<number> {
      const w = windowRef.current;
      if (!w.filter) return 0;
      try {
        // viewGeneration null = no snapshot pin = the live total.
        const latest = await countCommits({
          ...w.filter,
          viewGeneration: null,
        });
        return Math.max(0, latest - w.count);
      } catch {
        return 0;
      }
    }

    return { reconcile, reload, countNew, cancelPendingJump };
  }, []);

  const requestRange = useCallback(
    (first: number, last: number) => {
      lastRangeRef.current = { first, last };
      machinery.reconcile();
    },
    [machinery],
  );

  const reloadLatest = useCallback(() => {
    void machinery.reload("top", true, true);
  }, [machinery]);

  const countNew = useCallback(() => machinery.countNew(), [machinery]);

  // (Re)load whenever the filters or refreshNonce change. The first load
  // starts at the top; later ones recover around the focused commit.
  //
  // A background git→cache refill follows ONLY when fresh git data could
  // exist — the first load, a panel re-summon (`refreshNonce`), or a
  // changed time window. A repo / author chip change is a pure re-query of
  // the already-cached commits, so it skips the refill. The refill (and its
  // quiet in-place re-pull) is sequenced strictly AFTER the primary reload,
  // so it can never supersede that reload's anchor recovery.
  const filterKey = JSON.stringify([repoIds, authors, windowDays, refreshNonce]);
  const refillKeyRef = useRef<{ windowDays: number | null; refreshNonce: number }>(
    { windowDays, refreshNonce },
  );
  useEffect(() => {
    const isInitial = windowRef.current.filter == null;
    const prevRefill = refillKeyRef.current;
    const needsRefill =
      isInitial ||
      windowDays !== prevRefill.windowDays ||
      refreshNonce !== prevRefill.refreshNonce;
    refillKeyRef.current = { windowDays, refreshNonce };

    void machinery
      .reload(isInitial ? "top" : "recover", false, true)
      .then(() => {
        if (!needsRefill) return;
        const qid = queryRef.current;
        recentCommits(paramsRef.current.windowDays)
          .then(() => {
            if (qid === queryRef.current) {
              void machinery.reload("current", false, false);
            }
          })
          .catch(() => {});
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  // Clear the fresh markers when the panel loses focus — the user has
  // "seen" what was new. The delay + hasFocus check keeps a tray-menu or
  // chip-dropdown blur from wiping them mid-interaction.
  useEffect(() => {
    function onBlur() {
      window.setTimeout(() => {
        if (!document.hasFocus()) setFreshHashes(new Set());
      }, 200);
    }
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Drop a pending jump timer on unmount.
  useEffect(() => {
    return () => machinery.cancelPendingJump();
  }, [machinery]);

  return {
    rows,
    baseIndex,
    count,
    status,
    freshHashes,
    recovery,
    requestRange,
    reloadLatest,
    countNew,
  };
}
