// Single-repo timeline — virtualized DAG (lane graph).
//
// The repo's full commit list is held in memory (the lane algorithm has to
// thread parent links through the whole history), but only the rows in —
// and just around — the viewport are mounted as DOM; a top/bottom spacer
// stands in for the rest. The lane SVG is windowed the same way: only the
// nodes/edges intersecting the visible band are emitted. So a 100k-commit
// repo never puts 100k <li> + ~200k SVG nodes in the document.
//
// Lane geometry is COMPUTED from a fixed row height (plus the one measured
// inline expansion), never measured back from the DOM — the old per-row
// offsetTop sweep was an O(n) forced reflow on every layout change.

import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { colorForBranch } from "../lib/colors";
import {
  buildCommitMenuItems,
  copyCommitAiContext,
} from "../lib/commitClipboard";
import { computeLanes, type LaneEdge } from "../lib/lanes";
import { openDiff, prefetchCommit } from "../lib/ipc";
import type { BranchInfo, CommitSummary } from "../types";
import { ChangedFiles } from "./ChangedFiles";
import { CommitDetail } from "./CommitDetail";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { LaneGraph } from "./LaneGraph";

/** Fixed single-repo row height — mirrors `.timeline-single .timeline-row`
 * in styles.css. The virtualization math depends on this being exact. */
const ROW_H = 31;
/** Extra rows rendered above/below the viewport for scroll smoothness. */
const OVERSCAN = 8;
/** Row-index chunk size for the edge bucket index. The visible-edge query
 * then touches only the one or two chunks the viewport spans, keeping it
 * sub-linear in the repo's commit count. */
const EDGE_CHUNK = 512;

interface Props {
  /** The repo's full commit list, newest-first. */
  commits: CommitSummary[];
  /** Branch list — colors the DAG lanes by branch identity. */
  branches?: BranchInfo[];
}

function timeAgo(unixSeconds: number): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86_400)}d`;
}

/** Full local datetime for hover tooltips — 24-hour, 2-digit components in
 * the user's locale so it's unambiguous regardless of region. */
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

export function Timeline({ commits, branches }: Props) {
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
  // Measured pixel height of the one open inline expansion (0 = none) —
  // folded into the virtualization so the expanded row's extra height is
  // exact and the expansion survives scrolling.
  const [expansionH, setExpansionH] = useState(0);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const expansionObserver = useRef<ResizeObserver | null>(null);
  const hoverTimers = useRef(new Map<string, number>());

  const total = commits.length;

  // ----- lane layout: precomputed once over the full history -----
  const headBranch = branches?.find((b) => b.isHead)?.name ?? null;
  const graph = useMemo(
    () =>
      computeLanes(commits, (c) => colorForBranch(c.branchLabel ?? headBranch)),
    [commits, headBranch],
  );

  // Edge bucket index — each edge is filed under every row-index chunk it
  // spans. The visible-edge query then reads just the chunk(s) the viewport
  // covers instead of scanning every edge in the repo. Rebuilt only when the
  // commit list changes (the spans are index-space, expansion-independent).
  const edgeChunks = useMemo(() => {
    const chunks: number[][] = [];
    graph.edges.forEach((e, ei) => {
      const a = e.fromIdx;
      const b = e.toIdx >= 0 ? e.toIdx : total; // off-window parent → bottom
      const c0 = Math.floor(Math.min(a, b) / EDGE_CHUNK);
      const c1 = Math.floor(Math.max(a, b) / EDGE_CHUNK);
      for (let c = c0; c <= c1; c++) (chunks[c] ??= []).push(ei);
    });
    return chunks;
  }, [graph, total]);

  // ----- virtual range (the one open expansion folded into the geometry) ---
  // Global index of the open expansion's row, or -1 when nothing is open /
  // the expanded commit is no longer in the list. Memoized: with the full
  // commit array in hand this findIndex is O(n) — it must not re-run on
  // every scroll frame.
  const expandedIndex = useMemo(
    () =>
      expandedHash ? commits.findIndex((c) => c.hash === expandedHash) : -1,
    [commits, expandedHash],
  );
  const expH = expandedIndex >= 0 ? expansionH : 0;
  // content-Y of the top of row `i` — rows below the expansion sit `expH`
  // lower than their bare row-height position.
  const offsetOfRow = (i: number) =>
    i * ROW_H + (expandedIndex >= 0 && i > expandedIndex ? expH : 0);
  // inverse: the row index whose slot contains content-y `y`.
  const rowAtOffset = (y: number) => {
    if (expandedIndex < 0 || expH === 0) return Math.floor(y / ROW_H);
    const expTop = (expandedIndex + 1) * ROW_H;
    if (y < expTop) return Math.floor(y / ROW_H);
    if (y < expTop + expH) return expandedIndex;
    return expandedIndex + 1 + Math.floor((y - expTop - expH) / ROW_H);
  };
  const totalHeight = total * ROW_H + (expandedIndex >= 0 ? expH : 0);
  const first = Math.max(0, rowAtOffset(scrollTop) - OVERSCAN);
  const last = Math.min(
    total,
    rowAtOffset(scrollTop + viewportH) + OVERSCAN + 1,
  );
  const visible = commits.slice(first, last);
  const padTop = offsetOfRow(first);
  const padBottom = Math.max(0, totalHeight - offsetOfRow(last));
  const bandHeight = Math.max(0, offsetOfRow(last) - padTop);
  // content-Y centre of row `idx` — the lane SVG's node/edge anchor.
  const cy = (idx: number) => offsetOfRow(idx) + ROW_H / 2;
  // Ref mirror so the selection-scroll effect reads the live geometry
  // without depending on (and re-firing for) every expansion resize.
  const offsetOfRowRef = useRef(offsetOfRow);
  offsetOfRowRef.current = offsetOfRow;

  // Edges whose row span intersects the visible band — read from the bucket
  // index, then precisely re-tested (a chunk is coarser than the band).
  const visibleEdges = useMemo<LaneEdge[]>(() => {
    if (last <= first) return [];
    const c0 = Math.floor(first / EDGE_CHUNK);
    const c1 = Math.floor((last - 1) / EDGE_CHUNK);
    const seen = new Set<number>();
    const out: LaneEdge[] = [];
    for (let c = c0; c <= c1; c++) {
      const bucket = edgeChunks[c];
      if (!bucket) continue;
      for (const ei of bucket) {
        if (seen.has(ei)) continue;
        seen.add(ei);
        const e = graph.edges[ei];
        const a = e.fromIdx;
        const b = e.toIdx >= 0 ? e.toIdx : total;
        if (Math.min(a, b) < last && Math.max(a, b) >= first) out.push(e);
      }
    }
    return out;
  }, [edgeChunks, graph, total, first, last]);

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

  // Fresh commit list (repo / branch / window / author swap, or a re-pull)
  // — reset selection, expansion, and scroll back to the newest commit.
  useEffect(() => {
    setSelected(0);
    setExpandedHash(null);
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [commits]);

  // Keep `selected` in range as the commit list changes.
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

  // `ref` for the open expansion's <li>: measure its height (it grows as
  // ChangedFiles loads in) and keep `expansionH` current. Called with null
  // when the expansion unmounts (collapsed, or scrolled out of the band).
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

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (el) setScrollTop(el.scrollTop);
  }, []);

  // ----- keyboard nav (j / k / Enter / c / Esc) -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.key === "j" || e.key === "ArrowDown") {
        setSelected((s) => Math.min(s + 1, Math.max(0, total - 1)));
        e.preventDefault();
      } else if (e.key === "k" || e.key === "ArrowUp") {
        setSelected((s) => Math.max(s - 1, 0));
        e.preventDefault();
      } else if (e.key === "Enter") {
        const c = commits[selected];
        if (c) toggleExpand(c.hash);
        e.preventDefault();
      } else if (e.key === "c" || e.key === "C") {
        const c = commits[selected];
        if (c) {
          void copyAiContext(c);
          e.preventDefault();
        }
      } else if (e.key === "Escape" && expandedHash != null) {
        setExpandedHash(null);
        e.preventDefault();
        // Block App's panel-hide Esc handler when we've consumed the key.
        e.stopImmediatePropagation();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commits, selected, total, toggleExpand, expandedHash, copyAiContext]);

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

    // Direct hit on a timeline row wins; otherwise inherit from the
    // currently-expanded commit (the click is inside its expansion).
    const row = target.closest<HTMLLIElement>("[data-row]");
    const idx = row ? parseInt(row.dataset.row ?? "-1", 10) : -1;
    let commit: CommitSummary | null = idx >= 0 ? commits[idx] ?? null : null;
    if (!commit && expandedHash) {
      commit = commits.find((c) => c.hash === expandedHash) ?? null;
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
      {total === 0 ? (
        <p className="panel-empty">No commits match.</p>
      ) : (
        <ul
          className="timeline-windowed-list timeline-single"
          style={{ paddingTop: padTop, paddingBottom: padBottom }}
        >
          <LaneGraph
            laneCommits={graph.laneCommits}
            edges={visibleEdges}
            totalLanes={graph.totalLanes}
            first={first}
            last={last}
            cy={cy}
            bottomY={totalHeight}
            bandTop={padTop}
            bandHeight={bandHeight}
          />
          {visible.map((c, i) => {
            const idx = first + i;
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
                  <span className="timeline-lane-spacer" aria-hidden="true" />
                  <span
                    className="timeline-time"
                    title={formatFullTime(c.timestamp)}
                  >
                    {timeAgo(c.timestamp)}
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
              </Fragment>
            );
          })}
        </ul>
      )}
    </div>
  );
}
