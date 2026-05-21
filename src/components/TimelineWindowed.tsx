// All-repos timeline — windowed + virtualized.
//
// Backed by `useTimelineWindow`: rows are pulled one keyset page at a time
// from the cache as the user scrolls, never held in full. Only the rows in
// (and just around) the viewport are rendered as DOM — a top/bottom spacer
// stands in for the rest — so the timeline stays light no matter how many
// commits exist. Row height is fixed; the inline expansion collapses on
// scroll so the virtualization math stays exact.

import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import type { CommitSummary } from "../types";
import { buildCommitMenuItems, copyCommitAiContext } from "../lib/commitClipboard";
import { onTimelineInvalidated, openDiff, prefetchCommit } from "../lib/ipc";
import { useTimelineWindow } from "../lib/useTimelineWindow";
import { ChangedFiles } from "./ChangedFiles";
import { CommitDetail } from "./CommitDetail";
import { ContextMenu, type MenuItem } from "./ContextMenu";

/** Fixed all-repos row height — mirrors `.timeline-all .timeline-row` in
 * styles.css. The virtualization math depends on this being exact. */
const ROW_H = 31;
/** Extra rows rendered above/below the viewport for scroll smoothness. */
const OVERSCAN = 8;
/** Prefetch the next page when the loaded bottom is within this many
 * viewport-heights of the scroll position. */
const PREFETCH_VIEWPORTS = 1.5;
/** Collapse a burst of `timeline://invalidated` events into one reload. */
const INVALIDATE_DEBOUNCE_MS = 350;

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

export function TimelineWindowed({
  repoIds,
  authors,
  windowDays,
  refreshNonce,
  onSelectRepo,
}: Props) {
  const { rows, hasMore, status, loadingMore, loadMore, reloadSoft } =
    useTimelineWindow({ repoIds, authors, windowDays, refreshNonce });

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [selected, setSelected] = useState(0);
  const [expandedHash, setExpandedHash] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "error">(
    "idle",
  );
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);
  const hoverTimers = useRef(new Map<string, number>());

  // ----- virtual range -----
  const total = rows.length;
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const visibleCount = Math.ceil(viewportH / ROW_H) + OVERSCAN * 2;
  const last = Math.min(total, first + visibleCount);
  const visible = rows.slice(first, last);

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

  // Reset selection, expansion + scroll to the top when the filter set
  // changes — a fresh top-of-timeline reload.
  useEffect(() => {
    setSelected(0);
    setExpandedHash(null);
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [repoIds, authors, windowDays, refreshNonce]);

  // Keep `selected` in range as rows load / reload.
  useEffect(() => {
    if (selected > total - 1) setSelected(Math.max(0, total - 1));
  }, [total, selected]);

  const copyAiContext = useCallback(async (commit: CommitSummary) => {
    const result = await copyCommitAiContext(commit);
    setCopyStatus(result);
    setTimeout(() => setCopyStatus("idle"), result === "copied" ? 1500 : 2000);
  }, []);

  const toggleExpand = useCallback((hash: string) => {
    setExpandedHash((cur) => (cur === hash ? null : hash));
  }, []);

  // ----- scroll: virtual range + collapse-on-scroll + prefetch -----
  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    // Collapse the inline expansion the moment the user scrolls — keeps the
    // fixed-height virtualization math exact (Phase 5 moves detail to a
    // drawer and removes this constraint).
    setExpandedHash((cur) => (cur != null ? null : cur));
    if (
      hasMore &&
      el.scrollHeight - el.scrollTop - el.clientHeight <
        el.clientHeight * PREFETCH_VIEWPORTS
    ) {
      loadMore();
    }
  }, [hasMore, loadMore]);

  // If the loaded rows don't fill the viewport yet, keep pulling pages.
  useEffect(() => {
    if (
      status === "ready" &&
      hasMore &&
      !loadingMore &&
      total * ROW_H < viewportH + ROW_H
    ) {
      loadMore();
    }
  }, [status, hasMore, loadingMore, total, viewportH, loadMore]);

  // ----- scanner invalidation: debounced reload, but only at the top -----
  // Auto-refreshing a scrolled-down reader would yank the page out from
  // under them; the Phase 4 "N new" pill handles that case.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    let disposed = false;
    void onTimelineInvalidated(() => {
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        const el = scrollRef.current;
        if (el && el.scrollTop < ROW_H * 2) reloadSoft();
      }, INVALIDATE_DEBOUNCE_MS);
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });
    return () => {
      disposed = true;
      unlisten?.();
      if (timer) window.clearTimeout(timer);
    };
  }, [reloadSoft]);

  // ----- keyboard nav (j / k / Enter / c / Esc) -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.key === "j" || e.key === "ArrowDown") {
        setSelected((s) => Math.min(s + 1, total - 1));
        e.preventDefault();
      } else if (e.key === "k" || e.key === "ArrowUp") {
        setSelected((s) => Math.max(s - 1, 0));
        e.preventDefault();
      } else if (e.key === "Enter") {
        const c = rows[selected];
        if (c) toggleExpand(c.hash);
        e.preventDefault();
      } else if (e.key === "c" || e.key === "C") {
        const c = rows[selected];
        if (c) {
          void copyAiContext(c);
          e.preventDefault();
        }
      } else if (e.key === "Escape" && expandedHash != null) {
        setExpandedHash(null);
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [rows, selected, total, toggleExpand, expandedHash, copyAiContext]);

  // Bring the selected row into view (also feeds `onScroll`, so a keyboard
  // move past the loaded edge triggers a prefetch).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const top = selected * ROW_H;
    if (top < el.scrollTop) {
      el.scrollTop = top;
    } else if (top + ROW_H > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_H - el.clientHeight;
    }
  }, [selected]);

  // Clean up pending hover-prefetch timers on unmount.
  useEffect(() => {
    const timers = hoverTimers.current;
    return () => {
      for (const id of timers.values()) window.clearTimeout(id);
      timers.clear();
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
    let commit: CommitSummary | null = idx >= 0 ? rows[idx] ?? null : null;
    if (!commit && expandedHash) {
      commit = rows.find((c) => c.hash === expandedHash) ?? null;
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

  const showEmpty = status === "ready" && total === 0;

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
      {status === "error" ? (
        <p className="panel-empty">Couldn't load the timeline.</p>
      ) : status === "loading" && total === 0 ? (
        <p className="panel-empty">Loading commits…</p>
      ) : showEmpty ? (
        <p className="panel-empty">No commits match.</p>
      ) : (
        <ul
          className="timeline-windowed-list timeline-all"
          style={{
            paddingTop: first * ROW_H,
            paddingBottom: Math.max(0, total - last) * ROW_H,
          }}
        >
          {visible.map((c, i) => {
            const idx = first + i;
            const m = marker(c);
            return (
              <Fragment key={`${c.repoPath}:${c.hash}`}>
                <li
                  data-row={idx}
                  className={
                    "timeline-row" +
                    (idx === selected ? " selected" : "") +
                    (expandedHash === c.hash ? " expanded" : "")
                  }
                  onClick={() => {
                    setSelected(idx);
                    toggleExpand(c.hash);
                  }}
                  onMouseEnter={() => onRowEnter(c)}
                  onMouseLeave={() => onRowLeave(c)}
                >
                  <span className={"timeline-marker " + m.cls} title={m.title}>
                    {m.glyph}
                  </span>
                  <span
                    className="timeline-time"
                    title={formatFullTime(c.timestamp)}
                  >
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
                        {c.remoteTipExtraCount > 0 &&
                          ` +${c.remoteTipExtraCount}`}
                      </span>
                    )}
                  </span>
                  <span className="timeline-author" title={c.email}>
                    {c.author}
                  </span>
                </li>
                {expandedHash === c.hash && (
                  <li
                    className="timeline-expansion"
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
              </Fragment>
            );
          })}
        </ul>
      )}
      {loadingMore && (
        <div className="timeline-loading-more" aria-hidden="true">
          Loading…
        </div>
      )}
    </div>
  );
}
