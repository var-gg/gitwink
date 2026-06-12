// All-repos timeline — windowed + virtualized, with a random-access
// scrollbar.
//
// The scroll track is `count` rows tall — the full filtered timeline — so
// the native scrollbar represents the whole repo set and dragging its thumb
// jumps anywhere. Only ONE contiguous window of commits is held (see
// `useTimelineWindow`); it sits at its true global offset (`baseIndex`),
// rows outside it render as light placeholders until a load fills them in.
// On a filter change the viewport recovers around the previously-focused
// commit instead of snapping to the top.

import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { CommitSummary } from "../types";
import { colorForRepo } from "../lib/colors";
import {
  buildCommitMenuItems,
  copyCommitAiContext,
} from "../lib/commitClipboard";
import {
  changedFilesBatch,
  onTimelineInvalidated,
  openDiff,
  prefetchCommit,
} from "../lib/ipc";
import { useTimelineWindow } from "../lib/useTimelineWindow";
import { timelineRowH, useUiScale } from "../lib/settings";
import { ChangedFiles } from "./ChangedFiles";
import { CommitDetail } from "./CommitDetail";
import { ContextMenu, type MenuItem } from "./ContextMenu";

/** Extra rows rendered above/below the viewport for scroll smoothness. */
const OVERSCAN = 8;
/** Collapse a burst of `timeline://invalidated` events into one reload. */
const INVALIDATE_DEBOUNCE_MS = 350;

/** Imperative selection controls the search bar drives while keyboard
 * focus stays in its input (↑/↓/Enter act on the result rows). */
export interface SearchControl {
  moveSelection: (delta: number) => void;
  activateSelected: () => void;
}

interface Props {
  /** repo-id filter, or null for all repos */
  repoIds: number[] | null;
  /** author-name filter, or null for all authors */
  authors: string[] | null;
  /** time window in days, or null for all time */
  windowDays: number | null;
  /** bumped by App when the panel is re-summoned */
  refreshNonce: number;
  /** clicking the repo cell jumps to single-repo mode */
  onSelectRepo: (repoPath: string) => void;
  /** empty-state action: widen the time window to "All time" */
  onShowAllTime?: () => void;
  /** empty-state action: clear the author filter back to "all" */
  onClearAuthors?: () => void;
  /** empty-state action: open commit search — the moment someone stares
   *  at "no commits" hunting for one is exactly when they should learn
   *  search exists. */
  onOpenSearch?: () => void;
  /** free-text search filter — search-results rendering of the timeline */
  query?: string | null;
  /** pure cache re-query: never chain the git→cache refill (search view) */
  skipRefill?: boolean;
  /** Search-results mode: Enter warps via `onWarp` instead of expanding
   *  (click still previews inline), rows grow a jump affordance, and the
   *  empty state reads as a search miss. */
  searchMode?: boolean;
  /** warp to a commit's context — search-mode Enter / jump-button click */
  onWarp?: (c: CommitSummary) => void;
  /** filled with the imperative selection controls (see SearchControl) */
  searchControlRef?: React.MutableRefObject<SearchControl | null>;
  /** reports the filtered count (null while loading) — the bar's label */
  onResultCount?: (n: number | null) => void;
}

function timeAgo(unixSeconds: number): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86_400)}d`;
}

function formatFullTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function marker(c: CommitSummary): { glyph: string; cls: string; title: string } {
  if (c.isTagged) return { glyph: "★", cls: "marker-tag", title: "Tagged commit" };
  if (c.isMerge) return { glyph: "◆", cls: "marker-merge", title: "Merge commit" };
  return { glyph: "●", cls: "marker-dot", title: "Commit" };
}

/** Stable per-row identity — `repoPath:hash`. Distinct repos can share a
 * commit hash (forks / clones), so the all-repos timeline must key rows by
 * path+hash, never hash alone. */
function rowKey(c: { repoPath: string; hash: string }): string {
  return `${c.repoPath}:${c.hash}`;
}

export function TimelineWindowed({
  repoIds,
  authors,
  windowDays,
  refreshNonce,
  onSelectRepo,
  onShowAllTime,
  onClearAuthors,
  onOpenSearch,
  query,
  skipRefill,
  searchMode,
  onWarp,
  searchControlRef,
  onResultCount,
}: Props) {
  const {
    rows,
    baseIndex,
    count,
    status,
    freshHashes,
    recovery,
    requestRange,
    reloadLatest,
    reloadInPlace,
    countNew,
  } = useTimelineWindow({
    repoIds,
    authors,
    windowDays,
    refreshNonce,
    query,
    skipRefill,
  });

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  // `selected` is a GLOBAL row index (it can point at a not-yet-loaded row).
  const [selected, setSelected] = useState(0);
  const [expandedKey, setExpandedKey] = useState<string | null>(null);
  // Ref mirror of `expandedKey` so the long-lived invalidation listener
  // reads the live expanded state without re-subscribing on every toggle.
  const expandedKeyRef = useRef<string | null>(null);
  expandedKeyRef.current = expandedKey;
  // Ref mirror of `status` for the same listener — a delete-burst reload
  // must yield to an in-flight reload instead of cancelling it.
  const statusRef = useRef(status);
  statusRef.current = status;
  // Anchors for the follow-expanded effect: track the open commit's key, its
  // last global index, and the last recovery nonce so a list shift can be told
  // apart from a click / a filter-change recovery.
  const prevExpandedKeyRef = useRef<string | null>(null);
  const prevExpandedGiRef = useRef(-1);
  const prevRecoveryNonceRef = useRef(0);
  // "N new commits" pill — set when the scanner reports new commits while
  // the reader is scrolled away from the top.
  const [newCount, setNewCount] = useState(0);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "error">(
    "idle",
  );
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);
  const hoverTimers = useRef(new Map<string, number>());
  // Measured pixel height of the one open inline expansion (0 = none).
  const [expansionH, setExpansionH] = useState(0);
  const expansionObserver = useRef<ResizeObserver | null>(null);

  // All-repos row height tracks the UI-scale setting; the virtual geometry
  // below recomputes whenever it changes. ROW_H stays an exact integer the
  // CSS --timeline-row-h mirrors, so the JS + CSS geometry never drift.
  const ROW_H = timelineRowH(useUiScale());

  /** The commit at global index `gi`, or null when it is outside the
   *  loaded window (a placeholder row). */
  const commitAt = useCallback(
    (gi: number): CommitSummary | null => {
      const li = gi - baseIndex;
      return li >= 0 && li < rows.length ? rows[li] : null;
    },
    [rows, baseIndex],
  );

  // ----- virtual geometry (global-index space; the open expansion folded in)
  // Global index of the open expansion's row, or -1 when nothing is open /
  // the expanded commit is outside the loaded window.
  const expandedIndex = useMemo(() => {
    if (!expandedKey) return -1;
    const li = rows.findIndex((r) => rowKey(r) === expandedKey);
    return li >= 0 ? baseIndex + li : -1;
  }, [rows, baseIndex, expandedKey]);
  const expH = expandedIndex >= 0 ? expansionH : 0;
  // content-y of the top of global row `i`.
  const offsetOfRow = (i: number) =>
    i * ROW_H + (expandedIndex >= 0 && i > expandedIndex ? expH : 0);
  // inverse: the global row index whose slot contains content-y `y`.
  const rowAtOffset = (y: number) => {
    if (expandedIndex < 0 || expH === 0) return Math.floor(y / ROW_H);
    const expTop = (expandedIndex + 1) * ROW_H;
    if (y < expTop) return Math.floor(y / ROW_H);
    if (y < expTop + expH) return expandedIndex;
    return expandedIndex + 1 + Math.floor((y - expTop - expH) / ROW_H);
  };
  const totalHeight = count * ROW_H + (expandedIndex >= 0 ? expH : 0);
  const first = Math.max(0, rowAtOffset(scrollTop) - OVERSCAN);
  const last = Math.min(
    count,
    rowAtOffset(scrollTop + viewportH) + OVERSCAN + 1,
  );
  const padTop = offsetOfRow(first);
  const padBottom = Math.max(0, totalHeight - offsetOfRow(last));
  // Ref mirror so the selection-scroll / recovery effects read the live
  // geometry without depending on (and re-firing for) every resize.
  const offsetOfRowRef = useRef(offsetOfRow);
  offsetOfRowRef.current = offsetOfRow;

  // ----- viewport height (fixed panel, but observe to stay robust) -----
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => setViewportH(el.clientHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Tell the hook which global rows the viewport needs — it extends or
  // jump-loads the window to cover them. Fires on scroll, recovery, resize.
  useEffect(() => {
    requestRange(first, last);
  }, [first, last, requestRange]);

  // Scroll-to signal from the hook: initial load, filter-change recovery,
  // or reload-to-latest. Land the recovered row at the viewport top, and
  // collapse any open expansion — it belongs to the superseded view.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    setExpandedKey(null);
    // The recovery collapses any open expansion, so the post-recovery
    // geometry is plain rows — scroll by the bare row height. Read it back:
    // the browser clamps scrollTop to the track near the very end.
    el.scrollTop = recovery.index * ROW_H;
    setScrollTop(el.scrollTop);
    setSelected(recovery.index);
    setNewCount(0);
  }, [recovery]);

  // Keep `selected` in range as the count changes.
  useEffect(() => {
    if (selected > count - 1) setSelected(Math.max(0, count - 1));
  }, [count, selected]);

  // Follow the open commit across a quiet window refresh. New commits inserted
  // above shift its global index; keep selection AND the viewport offset on
  // it. Apply the scroll delta ONLY when the SAME commit stays open and its
  // index moved — a click (expandedKey change) or a filter-change recovery
  // (which deliberately re-anchors) just re-seeds. Layout effect so scroll
  // lands before paint.
  useLayoutEffect(() => {
    const recovered = prevRecoveryNonceRef.current !== recovery.nonce;
    const expandedChanged = prevExpandedKeyRef.current !== expandedKey;
    prevRecoveryNonceRef.current = recovery.nonce;
    prevExpandedKeyRef.current = expandedKey;

    if (expandedKey == null) {
      prevExpandedGiRef.current = -1;
      return;
    }
    const li = rows.findIndex((r) => rowKey(r) === expandedKey);
    if (li < 0) {
      prevExpandedGiRef.current = -1;
      return;
    }
    const gi = baseIndex + li;
    const prevGi = prevExpandedGiRef.current;
    prevExpandedGiRef.current = gi;

    if (recovered || expandedChanged || prevGi < 0) return;
    if (gi === prevGi) {
      setSelected((prev) => (prev === gi ? prev : gi));
      return;
    }
    setSelected(gi);
    const el = scrollRef.current;
    if (el) {
      el.scrollTop += (gi - prevGi) * ROW_H;
      setScrollTop(el.scrollTop);
    }
  }, [rows, baseIndex, expandedKey, recovery.nonce, ROW_H]);

  const copyAiContext = useCallback(async (commit: CommitSummary) => {
    const result = await copyCommitAiContext(commit);
    setCopyStatus(result);
    setTimeout(() => setCopyStatus("idle"), result === "copied" ? 1500 : 2000);
  }, []);

  const toggleExpand = useCallback((key: string) => {
    setExpandedKey((cur) => (cur === key ? null : key));
  }, []);

  // `ref` for the open expansion's <li>: measure its height (it grows as
  // ChangedFiles loads in) and keep `expansionH` current. Called with null
  // when the expansion unmounts (collapsed, or scrolled out of the window).
  const measureExpansion = useCallback((el: HTMLLIElement | null) => {
    expansionObserver.current?.disconnect();
    expansionObserver.current = null;
    if (!el) {
      setExpansionH(0);
      return;
    }
    setExpansionH(el.offsetHeight);
    const observer = new ResizeObserver(() => setExpansionH(el.offsetHeight));
    observer.observe(el);
    expansionObserver.current = observer;
  }, []);

  // ----- scroll -----
  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    // Reaching the top with a pending "N new" pill = "show me the latest".
    if (newCount > 0 && el.scrollTop < ROW_H * 2) {
      setNewCount(0);
      reloadLatest();
    }
  }, [newCount, reloadLatest]);

  // Click the "N new" pill — reload to the latest (the hook's recovery
  // signal scrolls back to the top).
  const handlePillClick = useCallback(() => {
    setNewCount(0);
    reloadLatest();
  }, [reloadLatest]);

  // Phase 6: prefetch the changed-file lists for the loaded rows in view so
  // expanding one is instant. Debounced; the backend skips cached commits.
  useEffect(() => {
    const batch: { repoPath: string; hash: string }[] = [];
    for (let gi = first; gi < last; gi++) {
      const c = commitAt(gi);
      if (c) batch.push({ repoPath: c.repoPath, hash: c.hash });
    }
    if (batch.length === 0) return;
    const timer = window.setTimeout(() => {
      void changedFilesBatch(batch);
    }, 200);
    return () => window.clearTimeout(timer);
  }, [first, last, commitAt]);

  // ----- scanner invalidation: debounced -----
  // At the top, auto-advance to the latest. Scrolled away, surface a
  // "N new" pill instead of yanking the page out from under the reader.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    let disposed = false;
    // Whether the debounced burst included reconciled deletions (history
    // was rewritten). Rows the reader is looking at may no longer exist,
    // so a mid-scroll view re-pulls in place instead of waiting for the
    // next summon — a stale "N new" pill is annoying, stale ghost commits
    // are a lie.
    let sawDeletes = false;
    void onTimelineInvalidated((p) => {
      if (p.deleted > 0) sawDeletes = true;
      if (timer) window.clearTimeout(timer);
      const fire = () => {
        if (disposed) return;
        // A reload is already in flight (filter change / summon). Firing
        // reloadInPlace now would bump the query id, cancel that reload,
        // and silently drop its chained git→cache refill — wait it out and
        // re-decide with this burst's flags intact.
        if (statusRef.current === "loading") {
          timer = window.setTimeout(fire, INVALIDATE_DEBOUNCE_MS);
          return;
        }
        const burstHadDeletes = sawDeletes;
        sawDeletes = false;
        void (async () => {
          const el = scrollRef.current;
          // At the very top with nothing expanded → auto-advance to the
          // latest. If a row is expanded the reader is mid-view: don't yank
          // the page out from under them — surface the "N new" pill instead
          // and let them opt in.
          if (
            el &&
            el.scrollTop < ROW_H * 2 &&
            expandedKeyRef.current == null
          ) {
            reloadLatest();
          } else if (burstHadDeletes) {
            // The re-pull re-pins generation + count, so any pending "N
            // new" pill is satisfied by the same pass — clear it rather
            // than leave a stale number floating.
            setNewCount(0);
            reloadInPlace();
          } else {
            const n = await countNew();
            if (!disposed && n > 0) setNewCount(n);
          }
        })();
      };
      timer = window.setTimeout(fire, INVALIDATE_DEBOUNCE_MS);
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });
    return () => {
      disposed = true;
      unlisten?.();
      if (timer) window.clearTimeout(timer);
    };
  }, [reloadLatest, reloadInPlace, countNew]);

  // ----- keyboard nav (j / k / Enter / c / Esc) -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.key === "j" || e.key === "ArrowDown") {
        setSelected((s) => Math.min(s + 1, Math.max(0, count - 1)));
        e.preventDefault();
      } else if (e.key === "k" || e.key === "ArrowUp") {
        setSelected((s) => Math.max(s - 1, 0));
        e.preventDefault();
      } else if (e.key === "Enter") {
        const c = commitAt(selected);
        // Search results: Enter is the warp ("take me to its history"),
        // matching the search bar's Enter. Click still previews inline.
        if (c) {
          if (searchMode && onWarp) onWarp(c);
          else toggleExpand(rowKey(c));
        }
        e.preventDefault();
      } else if (e.key === "c" || e.key === "C") {
        const c = commitAt(selected);
        if (c) {
          void copyAiContext(c);
          e.preventDefault();
        }
      } else if (e.key === "Escape" && expandedKey != null) {
        setExpandedKey(null);
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    commitAt,
    selected,
    count,
    toggleExpand,
    expandedKey,
    copyAiContext,
    searchMode,
    onWarp,
  ]);

  // Hand the search bar its selection bridge — its input keeps keyboard
  // focus, so ↑/↓/Enter arrive there and are forwarded through this.
  useEffect(() => {
    if (!searchControlRef) return;
    searchControlRef.current = {
      moveSelection: (delta) =>
        setSelected((s) =>
          Math.max(0, Math.min(s + delta, Math.max(0, count - 1))),
        ),
      activateSelected: () => {
        const c = commitAt(selected);
        if (c && onWarp) onWarp(c);
      },
    };
    return () => {
      searchControlRef.current = null;
    };
  }, [searchControlRef, count, commitAt, selected, onWarp]);

  // Surface the filtered total — the search bar's "N matches" label.
  useEffect(() => {
    onResultCount?.(status === "ready" ? count : null);
  }, [onResultCount, status, count]);

  // Bring the selected row into view. Uses the live row geometry so a
  // selection below the open expansion still lands right.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const top = offsetOfRowRef.current(selected);
    if (top < el.scrollTop) {
      el.scrollTop = top;
    } else if (top + ROW_H > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_H - el.clientHeight;
    }
  }, [selected]);

  // Clean up pending hover-prefetch timers + the expansion observer on unmount.
  useEffect(() => {
    const timers = hoverTimers.current;
    return () => {
      for (const id of timers.values()) window.clearTimeout(id);
      timers.clear();
      expansionObserver.current?.disconnect();
    };
  }, []);

  function onRowEnter(c: CommitSummary) {
    const key = `${c.repoPath}:${c.hash}`;
    if (hoverTimers.current.has(key)) return;
    const id = window.setTimeout(() => {
      void prefetchCommit(c.repoPath, c.hash);
      hoverTimers.current.delete(key);
    }, 200);
    hoverTimers.current.set(key, id);
  }

  function onRowLeave(c: CommitSummary) {
    const key = `${c.repoPath}:${c.hash}`;
    const id = hoverTimers.current.get(key);
    if (id != null) {
      window.clearTimeout(id);
      hoverTimers.current.delete(key);
    }
  }

  function onTimelineContextMenu(e: React.MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('input, textarea, [contenteditable="true"]')) return;
    e.preventDefault();

    const row = target.closest<HTMLLIElement>("[data-row]");
    const idx = row ? parseInt(row.dataset.row ?? "-1", 10) : -1;
    let commit: CommitSummary | null = idx >= 0 ? commitAt(idx) : null;
    if (!commit && expandedKey) {
      commit = rows.find((c) => rowKey(c) === expandedKey) ?? null;
    }
    const fileEl = target.closest<HTMLElement>("[data-file-path]");
    const filePath = fileEl?.dataset.filePath ?? null;
    const selection = window.getSelection()?.toString() ?? "";

    const items = buildCommitMenuItems({
      commit,
      filePath,
      selection,
      onCopyAiContext: (c) => void copyAiContext(c),
    });
    if (items.length === 0) return;
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  }

  const showEmpty = status === "ready" && count === 0;

  // Build the visible slice: real rows where the window covers the global
  // index, light placeholders elsewhere (a jump-load in flight).
  const items: React.ReactNode[] = [];
  for (let gi = first; gi < last; gi++) {
    const c = commitAt(gi);
    if (!c) {
      items.push(
        <li
          key={`blank-${gi}`}
          className="timeline-row timeline-row-blank"
          aria-hidden="true"
        >
          <span className="timeline-skeleton" />
        </li>,
      );
      continue;
    }
    const m = marker(c);
    const key = rowKey(c);
    items.push(
      <Fragment key={key}>
        <li
          data-row={gi}
          className={
            "timeline-row" +
            (gi === selected ? " selected" : "") +
            (expandedKey === key ? " expanded" : "")
          }
          onClick={() => {
            setSelected(gi);
            toggleExpand(key);
          }}
          onMouseEnter={() => onRowEnter(c)}
          onMouseLeave={() => onRowLeave(c)}
        >
          <span
            className={
              "timeline-marker " +
              m.cls +
              (freshHashes.has(key) ? " fresh" : "")
            }
            title={freshHashes.has(key) ? `${m.title} (new)` : m.title}
          >
            {m.glyph}
          </span>
          <span className="timeline-time" title={formatFullTime(c.timestamp)}>
            {timeAgo(c.timestamp)}
          </span>
          <span
            className="timeline-repo timeline-repo-clickable"
            title={`${c.repoPath} (click to filter)`}
            onClick={(e) => {
              e.stopPropagation();
              onSelectRepo(c.repoPath);
            }}
          >
            <span
              className="timeline-repo-dot"
              style={{ background: colorForRepo(c.repoPath) }}
              aria-hidden="true"
            />
            {c.repoName}
          </span>
          <span className="timeline-summary" title={c.summary}>
            {c.branchLabel && (
              <span className="timeline-branch">[{c.branchLabel}]</span>
            )}
            {c.summary}
            {c.remoteTipLabel && (
              <span
                className="timeline-remote-tip"
                title={
                  c.remoteTipExtraCount > 0
                    ? `Remote tracking refs at this commit (read-only, from your last git fetch). +${c.remoteTipExtraCount} more.`
                    : "Remote tracking ref at this commit (read-only, from your last git fetch)."
                }
              >
                {c.remoteTipLabel}
                {c.remoteTipExtraCount > 0 && ` +${c.remoteTipExtraCount}`}
              </span>
            )}
          </span>
          <span className="timeline-author" title={c.email}>
            {c.author}
          </span>
          {searchMode && (
            <button
              type="button"
              className="row-jump"
              title="Jump to this commit in its repo's history (Enter)"
              onClick={(e) => {
                e.stopPropagation();
                onWarp?.(c);
              }}
            >
              ↗
            </button>
          )}
        </li>
        {expandedKey === key && (
          <li
            className="timeline-expansion"
            ref={measureExpansion}
            onClick={(e) => e.stopPropagation()}
          >
            <CommitDetail commit={c} />
            <div className="commit-actions">
              <button
                type="button"
                className="commit-copy-btn"
                onClick={() => void copyAiContext(c)}
                title="Copy as AI context (c)"
              >
                {copyStatus === "copied"
                  ? "Copied ✓"
                  : copyStatus === "error"
                    ? "Copy failed"
                    : "Copy as AI context"}
              </button>
            </div>
            <ChangedFiles
              repoPath={c.repoPath}
              hash={c.hash}
              onOpenDiff={(f) => {
                void openDiff(
                  c.repoPath,
                  c.repoName,
                  c.hash,
                  c.shortHash,
                  c.summary,
                  f.path,
                );
              }}
            />
          </li>
        )}
      </Fragment>,
    );
  }

  return (
    <div
      className="timeline-scroll"
      ref={scrollRef}
      onScroll={onScroll}
      onContextMenu={onTimelineContextMenu}
    >
      {contextMenu && (
        <ContextMenu
          items={contextMenu.items}
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
        />
      )}
      {newCount > 0 && (
        <div className="timeline-new-pill-anchor">
          <button
            type="button"
            className="timeline-new-pill"
            onClick={handlePillClick}
            title="Jump to the latest commits"
          >
            ↑ {newCount} new commit{newCount === 1 ? "" : "s"}
          </button>
        </div>
      )}
      {/* Anchor stays mounted so the aria-live region exists BEFORE content
          arrives — screen readers only announce changes inside an existing
          live region, not regions mounted together with their content. */}
      <div className="timeline-copy-toast-anchor" aria-live="polite">
        {copyStatus !== "idle" && (
          <span
            className={
              "timeline-copy-toast" + (copyStatus === "error" ? " error" : "")
            }
          >
            {copyStatus === "copied"
              ? "Copied as AI context ✓"
              : "Copy failed — try again"}
          </span>
        )}
      </div>
      {status === "error" ? (
        <p className="panel-empty">Couldn't load the timeline.</p>
      ) : status === "loading" && count === 0 ? (
        <p className="panel-empty">Loading commits…</p>
      ) : showEmpty && searchMode ? (
        // Search miss — the scan-window scope is the honest caveat (an
        // old commit can live outside the cache; a full SHA will get a
        // direct-lookup fallback in a later phase).
        <p className="panel-empty">No matches in scanned history.</p>
      ) : showEmpty ? (
        <div className="panel-empty">
          <p className="panel-empty-line">
            {windowDays != null
              ? windowDays === 1
                ? "No commits in the last 24 hours."
                : `No commits in the last ${windowDays} days.`
              : "No commits match."}
            {authors != null &&
              ` Filtered to ${authors.length} author${authors.length === 1 ? "" : "s"}.`}
          </p>
          {(windowDays != null || authors != null || onOpenSearch) && (
            <p className="panel-empty-actions">
              {windowDays != null && onShowAllTime && (
                <button
                  type="button"
                  className="panel-empty-action"
                  onClick={onShowAllTime}
                >
                  Show all time
                </button>
              )}
              {authors != null && onClearAuthors && (
                <button
                  type="button"
                  className="panel-empty-action"
                  onClick={onClearAuthors}
                >
                  Clear author filter
                </button>
              )}
              {onOpenSearch && (
                <button
                  type="button"
                  className="panel-empty-action"
                  onClick={onOpenSearch}
                  title="Search commits — message, author, SHA (/)"
                >
                  Search commits
                </button>
              )}
            </p>
          )}
        </div>
      ) : (
        <ul
          className={
            "timeline-windowed-list timeline-all" +
            (searchMode ? " timeline-search-list" : "")
          }
          style={{ paddingTop: padTop, paddingBottom: padBottom }}
        >
          {items}
        </ul>
      )}
    </div>
  );
}
