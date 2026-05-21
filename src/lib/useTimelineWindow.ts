// Windowed-pull data layer for the all-repos timeline.
//
// The all-repos timeline can span an unbounded number of commits, so the
// frontend never holds the full set. This hook keeps a contiguous slice
// loaded from the newest commit downward, fetched one keyset page at a
// time as the user scrolls. It pins a `viewGeneration` so the background
// scanner's concurrent inserts never disturb the page sequence, and tags
// every fetch with a query id so stale IPC responses (from a superseded
// filter / reload) are discarded.

import { useCallback, useEffect, useRef, useState } from "react";

import type { CommitSummary, Cursor, TimelineFilters } from "../types";
import {
  countCommits,
  getTimelineGeneration,
  listCommitsWindow,
  recentCommits,
} from "./ipc";

/** Rows fetched per keyset page — a few panel-heights so a fast scroll
 * doesn't outrun the loader. */
const PAGE_SIZE = 60;

export interface TimelineWindowParams {
  /** repo-id filter, or null for all repos */
  repoIds: number[] | null;
  /** author-name filter, or null for all authors */
  authors: string[] | null;
  /** time window in days, or null for all time */
  windowDays: number | null;
  /** bumped by the caller to force a full reload (panel re-summoned) */
  refreshNonce: number;
}

export interface TimelineWindowState {
  /** the contiguous slice loaded from the newest commit downward */
  rows: CommitSummary[];
  /** total commits under the filters — drives the count label */
  count: number;
  /** another keyset page exists below the loaded rows */
  hasMore: boolean;
  status: "loading" | "ready" | "error";
  /** a `loadMore` page fetch is in flight */
  loadingMore: boolean;
  /** append the next keyset page (no-op while loading / exhausted) */
  loadMore: () => void;
  /** re-pin the generation and reload from the top, WITHOUT a git refill —
   * for `timeline://invalidated` events (the watcher already wrote the
   * cache) and any other "data changed under us" trigger. */
  reloadSoft: () => void;
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000);
}

/** Live pagination cursor, mirrored out of React state so `loadMore` reads
 * the current values without being re-created on every render. */
interface PageRef {
  endCursor: Cursor | null;
  filter: TimelineFilters | null;
  hasMore: boolean;
  loadingMore: boolean;
}

export function useTimelineWindow(
  params: TimelineWindowParams,
): TimelineWindowState {
  const { repoIds, authors, windowDays, refreshNonce } = params;

  const [rows, setRows] = useState<CommitSummary[]>([]);
  const [count, setCount] = useState(0);
  const [status, setStatus] = useState<"loading" | "ready" | "error">(
    "loading",
  );
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);

  const pageRef = useRef<PageRef>({
    endCursor: null,
    filter: null,
    hasMore: false,
    loadingMore: false,
  });

  // Monotonic query id. Every (re)load bumps it; an in-flight response
  // whose id is stale is dropped — this is the stale-IPC-response guard.
  const queryRef = useRef(0);

  // Latest params, so the stable `doLoad` closure reads current values.
  const paramsRef = useRef(params);
  paramsRef.current = params;

  const doLoad = useCallback(async (kickRefill: boolean) => {
    const p = paramsRef.current;
    const qid = ++queryRef.current;
    setStatus("loading");

    if (kickRefill) {
      // Background git→cache refill. When it lands, reload once more (no
      // second refill) so the freshly-pinned generation sees its commits.
      recentCommits(p.windowDays)
        .then(() => {
          if (qid === queryRef.current) void doLoad(false);
        })
        .catch(() => {});
    }

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
      const [cnt, win] = await Promise.all([
        countCommits(filter),
        listCommitsWindow(filter, null, "older", PAGE_SIZE),
      ]);
      if (qid !== queryRef.current) return;
      pageRef.current = {
        endCursor: win.endCursor,
        filter,
        hasMore: win.hasOlder,
        loadingMore: false,
      };
      setCount(cnt);
      setRows(win.rows);
      setHasMore(win.hasOlder);
      setLoadingMore(false);
      setStatus("ready");
    } catch {
      if (qid === queryRef.current) setStatus("error");
    }
  }, []);

  const loadMore = useCallback(() => {
    const pg = pageRef.current;
    if (pg.loadingMore || !pg.hasMore || !pg.endCursor || !pg.filter) return;
    pg.loadingMore = true;
    setLoadingMore(true);
    const qid = queryRef.current;
    const cursor = pg.endCursor;
    const filter = pg.filter;
    void (async () => {
      try {
        const win = await listCommitsWindow(filter, cursor, "older", PAGE_SIZE);
        if (qid !== queryRef.current) return; // a reload superseded this page
        pageRef.current.endCursor = win.endCursor ?? pageRef.current.endCursor;
        pageRef.current.hasMore = win.hasOlder;
        setRows((prev) => [...prev, ...win.rows]);
        setHasMore(win.hasOlder);
      } catch {
        // Transient — leave hasMore set so a later scroll retries.
      } finally {
        if (qid === queryRef.current) {
          pageRef.current.loadingMore = false;
          setLoadingMore(false);
        }
      }
    })();
  }, []);

  const reloadSoft = useCallback(() => {
    void doLoad(false);
  }, [doLoad]);

  // (Re)load from the top whenever the filters or refreshNonce change.
  // The key string absorbs the referential instability of the array props.
  const filterKey = JSON.stringify([repoIds, authors, windowDays, refreshNonce]);
  useEffect(() => {
    void doLoad(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  return { rows, count, hasMore, status, loadingMore, loadMore, reloadSoft };
}
