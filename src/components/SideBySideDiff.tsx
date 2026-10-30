import {
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useLingui } from "@lingui/react/macro";
import type { Highlighter } from "shiki";

import { type DiffSide } from "../lib/diff";
import {
  longestLines,
  searchDiffRows,
  type FindMatch,
} from "../lib/diffView";
import {
  getHighlighter,
  highlightLineIfCached,
  highlightLineQueued,
  langForPath,
} from "../lib/highlight";
import { sbsHeaderH, sbsLineH, useUiScale } from "../lib/settings";
import { DiffMinimap } from "./DiffMinimap";
import { useParsedDiff } from "../lib/useParsedDiff";
import { diffMessages } from "../i18n/messages/diff";

interface Props {
  text: string;
  /** File path so we can detect the language for Shiki. Optional — falls
   * back to plain monospace when missing or unknown. */
  filePath?: string;
  /** Identity of the file this text belongs to (repo:hash:path). When `text`
   * changes under the SAME key — a ±3/±25/Full context toggle — the reading
   * position is re-anchored by line number instead of resetting to the top. */
  fileKey?: string;
  /** When true, the two columns scroll vertically as one (default). When
   * false they scroll independently — the overview rail then shows both
   * viewports and offers a re-align. */
  locked: boolean;
}

/** Rows mounted above & below the viewport so a fast flick never shows a gap
 * before the next window resolves. */
const OVERSCAN = 8;

/** Render cap for pathological single-line files (minified bundles): a
 * multi-MB line would otherwise lay out a multi-million-px text run in BOTH
 * columns — a multi-second main-thread stall with Esc unresponsive — and the
 * width probe would size the track past the browser's layout limits. Only
 * the RENDERING truncates (with a marker); the patch text itself is intact
 * ("Copy file diff" still yields everything). */
const LINE_RENDER_CAP = 10_000;

/** Skip syntax highlighting for lines longer than this (VS Code's
 * tokenization-limit pattern) — Shiki on a huge line is pure stall, and
 * plain text renders instantly. */
const HL_LINE_CAP = 2_000;

function isDarkScheme(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-color-scheme: dark)").matches
  );
}

// Persisted old/new split — fraction of width given to the left (old)
// column. Clamped so neither side can be dragged to nothing.
// The find query/open state survives file switches within the diff window's
// session (the component remounts per file; only one instance exists at a
// time) — hunting one symbol across a commit's files keeps the query.
let persistedFindQuery = "";
let persistedFindOpen = false;

const SPLIT_KEY = "gitwink.diffSplit";
const SPLIT_MIN = 0.15;
const SPLIT_MAX = 0.85;
function loadSplit(): number {
  if (typeof window === "undefined") return 0.5;
  const v = Number(window.localStorage.getItem(SPLIT_KEY));
  return Number.isFinite(v) && v >= SPLIT_MIN && v <= SPLIT_MAX ? v : 0.5;
}
function saveSplit(v: number): void {
  try {
    window.localStorage.setItem(SPLIT_KEY, String(v));
  } catch {}
}

export function SideBySideDiff({ text, filePath, fileKey, locked }: Props) {
  const { i18n } = useLingui();
  const scale = useUiScale();
  const lineH = sbsLineH(scale);
  const headerH = sbsHeaderH(scale);

  // Parse → flat items + overview segments. Memoized so dragging the splitter
  // (which we keep off React entirely, below) or a scroll never reparses.
  const { items, segments, parseError, parsing } = useParsedDiff(text);
  const hasRows = items.length > 0 && !parseError;

  // Prefix-sum of row tops: offsets[i] is the pixel top of row i, offsets[n]
  // the total height. Plain numbers, O(n) rebuild — cheap for tens of
  // thousands of rows. The SAME integers drive the inline row `height`.
  const offsets = useMemo(() => {
    const arr = new Array<number>(items.length + 1);
    arr[0] = 0;
    for (let i = 0; i < items.length; i++) {
      arr[i + 1] = arr[i] + (items[i].kind === "header" ? headerH : lineH);
    }
    return arr;
  }, [items, headerH, lineH]);
  const total = offsets[items.length];

  const { probeL, probeR } = useMemo(() => longestLines(items), [items]);

  const colsRef = useRef<HTMLDivElement | null>(null);
  const leftRef = useRef<HTMLDivElement | null>(null);
  const rightRef = useRef<HTMLDivElement | null>(null);
  const draggingRef = useRef(false);

  // Viewport height (both columns share the grid row, so one measure covers
  // both). A ResizeObserver keeps it correct across window/panel resizes.
  const [viewportH, setViewportH] = useState(0);
  useLayoutEffect(() => {
    const el = colsRef.current;
    if (!el) return;
    const measure = () => setViewportH(el.clientHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [hasRows]);

  // Each column's scroll position drives its own virtual window. Locked keeps
  // them in lockstep; unlocked lets the old/new sides roam.
  const [scrollTopL, setScrollTopL] = useState(0);
  const [scrollTopR, setScrollTopR] = useState(0);

  const [highlighter, setHighlighter] = useState<Highlighter | null>(null);
  const [dark, setDark] = useState(isDarkScheme);
  const [split, setSplit] = useState(loadSplit);
  // Mirror for the pointer handlers (avoids a stale split in the closure) and
  // to carry the live drag value into the release handler.
  const splitRef = useRef(split);
  splitRef.current = split;

  const lang = filePath ? langForPath(filePath) : null;

  // ----- reading-position anchor -----
  // Live geometry for the (stable) scroll listeners' top-line capture.
  const geomRef = useRef({ items, offsets });
  geomRef.current = { items, offsets };
  const anchorFileKeyRef = useRef<string | undefined>(undefined);
  type TopLine = { line: number; side: "left" | "right" } | null;
  const topLineRef = useRef<TopLine>(null);
  // The right column tracked separately, so an UNLOCKED right side
  // re-anchors to its own reading position, not the left's.
  const topLineRightRef = useRef<TopLine>(null);

  // Remember each column's top visible line (by old/new line NUMBER — those
  // survive a context change) on every scroll, so a same-file text swap can
  // restore the reading position.
  const topLineFor = (
    el: HTMLDivElement | null,
    prefer: "left" | "right",
  ): TopLine => {
    if (!el) return null;
    const { items: its, offsets: offs } = geomRef.current;
    if (its.length === 0) return null;
    const y = el.scrollTop;
    let lo = 0;
    let hi = its.length - 1;
    let ans = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (offs[mid] <= y) {
        ans = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    for (let i = ans; i < Math.min(its.length, ans + 60); i++) {
      const it = its[i];
      if (it.kind !== "row") continue;
      const first = prefer === "left" ? it.left : it.right;
      const second = prefer === "left" ? it.right : it.left;
      if (first.lineNum != null) {
        return { line: first.lineNum, side: prefer };
      }
      if (second.lineNum != null) {
        return {
          line: second.lineNum,
          side: prefer === "left" ? "right" : "left",
        };
      }
    }
    return null;
  };
  const captureTopLine = () => {
    topLineRef.current = topLineFor(leftRef.current, "left");
    topLineRightRef.current = topLineFor(rightRef.current, "right");
  };

  // Same-file text swap (a ±3/±25/Full context toggle, kept mounted by
  // DiffApp): re-anchor to the previous top line instead of yanking the
  // reader back to line 1. A DIFFERENT file opens at the top — the persisted
  // webview would otherwise carry an unrelated file's scroll over.
  useLayoutEffect(() => {
    const sameFile = fileKey != null && anchorFileKeyRef.current === fileKey;
    anchorFileKeyRef.current = fileKey;
    const idxOf = (a: TopLine): number => {
      if (!a) return -1;
      const { line, side } = a;
      return items.findIndex(
        (it) =>
          it.kind === "row" &&
          (side === "left"
            ? it.left.lineNum != null && it.left.lineNum >= line
            : it.right.lineNum != null && it.right.lineNum >= line),
      );
    };
    let topL = 0;
    let topR = 0;
    if (sameFile) {
      const iL = idxOf(topLineRef.current);
      if (iL > 0) topL = offsets[iL];
      topR = topL;
      // Unlocked columns keep their own positions across the swap.
      if (!locked) {
        const iR = idxOf(topLineRightRef.current);
        if (iR > 0) topR = offsets[iR];
      }
    } else {
      topLineRef.current = null;
      topLineRightRef.current = null;
    }
    if (leftRef.current) leftRef.current.scrollTop = topL;
    if (rightRef.current) rightRef.current.scrollTop = topR;
    setScrollTopL(topL);
    setScrollTopR(topR);
    // Worker results arrive after text changes; anchor the completed rows.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, fileKey]);

  // ----- in-diff find (Ctrl+F) -----
  // The columns are virtualized, so the browser's native find can't see
  // off-screen rows. This searches the flat item list instead and drives the
  // virtual scroll to each hit. Query/open state persists across file
  // switches via the module-scope mirror above; matches recompute per file.
  const [findOpen, setFindOpen] = useState(() => persistedFindOpen);
  const [findQuery, setFindQuery] = useState(() => persistedFindQuery);
  const [findCur, setFindCur] = useState(0);
  useEffect(() => {
    persistedFindQuery = findQuery;
  }, [findQuery]);
  useEffect(() => {
    persistedFindOpen = findOpen;
  }, [findOpen]);
  const findInputRef = useRef<HTMLInputElement | null>(null);
  const findOpenRef = useRef(false);
  findOpenRef.current = findOpen;
  // Live mirror for the mount-level key listener: a hunk-less diff (pure
  // rename / mode change) renders the "No textual diff." early return and no
  // bar — Ctrl+F must not arm invisible find state there (it would swallow
  // the next Esc meant for the window).
  const hasRowsRef = useRef(false);
  hasRowsRef.current = !!hasRows;

  const findMatches = useMemo(
    () => searchDiffRows(items, findQuery),
    [items, findQuery],
  );
  // Decorations are gated on the bar being OPEN — closing the bar clears all
  // tints (like every find UI) while the query survives for a re-open.
  const findMatchSet = useMemo(
    () =>
      findOpen
        ? new Set(findMatches.map((m) => m.row))
        : new Set<number>(),
    [findMatches, findOpen],
  );

  // Ctrl/Cmd+F summons (or refocuses) the bar; Esc closes it. Window-level —
  // the scroll columns aren't focusable — and CAPTURE phase, so the Esc that
  // closes the bar runs before (and suppresses) DiffApp's bubble-phase Esc
  // that hides the whole window, no matter where focus sits (a find button,
  // the diff text). e.code covers non-Latin layouts where e.key isn't "f".
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.isComposing) return; // IME mid-composition — never intercept
      if (!hasRowsRef.current) return; // empty diff renders no bar — stay inert
      if (
        (e.ctrlKey || e.metaKey) &&
        (e.key === "f" || e.key === "F" || e.code === "KeyF")
      ) {
        e.preventDefault();
        setFindOpen(true);
        requestAnimationFrame(() => {
          findInputRef.current?.focus();
          findInputRef.current?.select();
        });
        return;
      }
      if (e.key === "Escape" && findOpenRef.current) {
        // A context menu is the topmost layer — its own Esc handler must win;
        // this capture listener would otherwise close the find bar UNDER it.
        if (document.querySelector(".context-menu")) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        setFindOpen(false);
      }
    }
    window.addEventListener("keydown", onKey, { capture: true });
    return () =>
      window.removeEventListener("keydown", onKey, { capture: true });
  }, []);

  // Cached monospace glyph width for the horizontal reveal — re-measured only
  // when the computed font signature changes (UI scale / font setting).
  const charWRef = useRef<{ font: string; w: number } | null>(null);
  const measureCharW = (): number => {
    const el = leftRef.current;
    if (!el) return 7.2;
    const cs = window.getComputedStyle(el);
    const font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
    if (charWRef.current?.font === font) return charWRef.current.w;
    const ctx = document.createElement("canvas").getContext("2d");
    if (!ctx) return 7.2;
    ctx.font = font;
    const w = ctx.measureText("0".repeat(64)).width / 64;
    charWRef.current = { font, w };
    return w;
  };

  // Bring a match into view in both columns (unlocked sides get aligned on
  // purpose — a jump is a deliberate "take me there"): vertical to the upper
  // third, and HORIZONTAL to the match column — a hit past ~100 columns on a
  // long line would otherwise stay off-screen after the vertical jump.
  // Monospace metrics make col×charW exact for ASCII; wide glyphs still land
  // within the revealed third. Near-zero columns pin fully left.
  const scrollToMatch = (m: FindMatch) => {
    const top = Math.max(
      0,
      offsets[m.row] - Math.max(0, (viewportH - lineH) / 3),
    );
    const l = leftRef.current;
    const left =
      m.col <= 4 || !l
        ? 0
        : Math.max(0, m.col * measureCharW() - l.clientWidth / 3);
    for (const el of [leftRef.current, rightRef.current]) {
      if (!el) continue;
      el.scrollTop = top;
      el.scrollLeft = left;
    }
  };

  // New QUERY: restart at the first hit and bring it into view (only while
  // the bar is up — a closed find never moves the scroll). Gated on the query
  // actually changing: a same-query items refresh (mount with a persisted
  // query, or a context toggle) must NOT yank the reading position.
  const prevFindQueryRef = useRef(findQuery);
  useEffect(() => {
    setFindCur(0);
    const queryChanged = prevFindQueryRef.current !== findQuery;
    prevFindQueryRef.current = findQuery;
    if (!queryChanged) return;
    if (findOpenRef.current && findMatches.length > 0) {
      scrollToMatch(findMatches[0]);
    }
    // scrollToMatch reads memoized geometry; findMatches is the trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findMatches, findQuery]);

  const gotoFindMatch = (delta: number) => {
    if (findMatches.length === 0) return;
    const next = (findCur + delta + findMatches.length) % findMatches.length;
    setFindCur(next);
    scrollToMatch(findMatches[next]);
  };

  // Lazy-load Shiki on first mount that has a known language. Skipped
  // entirely for unknown extensions — saves a multi-MB download.
  useEffect(() => {
    if (!lang) return;
    let cancelled = false;
    void getHighlighter().then((hl) => {
      if (!cancelled) setHighlighter(hl);
    });
    return () => {
      cancelled = true;
    };
  }, [lang]);

  // React to OS theme changes while the window is open.
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setDark(mq.matches);
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, []);

  // Scroll sync + window tracking. Horizontal is always mirrored; vertical
  // only when `locked`. The scrolled position is pushed into state (rAF-
  // throttled, so a fast wheel doesn't setState per event) so the virtual
  // window follows. A `syncing` guard stops the mirrored write from echoing.
  useEffect(() => {
    const l = leftRef.current;
    const r = rightRef.current;
    if (!l || !r) return;
    if (locked) r.scrollTop = l.scrollTop;
    let syncing = false;
    let rafL = 0;
    let rafR = 0;
    const onL = () => {
      if (!syncing) {
        syncing = true;
        r.scrollLeft = l.scrollLeft;
        if (locked) r.scrollTop = l.scrollTop;
        syncing = false;
      }
      if (!rafL) {
        rafL = requestAnimationFrame(() => {
          rafL = 0;
          setScrollTopL(l.scrollTop);
          if (locked) setScrollTopR(l.scrollTop);
          captureTopLine();
        });
      }
    };
    const onR = () => {
      if (!syncing) {
        syncing = true;
        l.scrollLeft = r.scrollLeft;
        if (locked) l.scrollTop = r.scrollTop;
        syncing = false;
      }
      if (!rafR) {
        rafR = requestAnimationFrame(() => {
          rafR = 0;
          setScrollTopR(r.scrollTop);
          if (locked) setScrollTopL(r.scrollTop);
          captureTopLine();
        });
      }
    };
    l.addEventListener("scroll", onL, { passive: true });
    r.addEventListener("scroll", onR, { passive: true });
    return () => {
      l.removeEventListener("scroll", onL);
      r.removeEventListener("scroll", onR);
      if (rafL) cancelAnimationFrame(rafL);
      if (rafR) cancelAnimationFrame(rafR);
    };
  }, [locked, hasRows]);

  // Column resizer — drag the divider to rebalance old vs new, double-click
  // to reset. We update a CSS variable directly via ref during the drag so the
  // diff never re-renders mid-drag (the killer for a big file); state + persist
  // only land on release. Pointer capture keeps the drag alive over the columns.
  function applySplitVar(v: number) {
    const el = colsRef.current;
    if (el) {
      el.style.setProperty("--sbs-l", `${v}fr`);
      el.style.setProperty("--sbs-r", `${1 - v}fr`);
    }
  }
  function onResizerDown(e: React.PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    draggingRef.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function onResizerMove(e: React.PointerEvent) {
    if (!draggingRef.current || !colsRef.current) return;
    const rect = colsRef.current.getBoundingClientRect();
    if (rect.width <= 0) return;
    const r = (e.clientX - rect.left) / rect.width;
    if (!Number.isFinite(r)) return;
    const clamped = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, r));
    splitRef.current = clamped;
    applySplitVar(clamped); // no setState — zero re-render during the drag
  }
  function finishDrag(e: React.PointerEvent) {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {}
    setSplit(splitRef.current); // commit once, after the drag
    saveSplit(splitRef.current);
  }

  if (parseError) return <div className="diff-loading" role="alert">{i18n._(diffMessages.loadFailed)}</div>;

  if (parsing && items.length === 0) return <div className="diff-loading" role="status">{i18n._(diffMessages.loading)}</div>;

  if (items.length === 0) {
    return <div className="sbs-empty">{i18n._(diffMessages.noText)}</div>;
  }

  // Largest row index whose top edge is <= y. Binary search → O(log n) per tick.
  const rowAt = (y: number) => {
    let lo = 0;
    let hi = items.length - 1;
    let ans = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (offsets[mid] <= y) {
        ans = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    return ans;
  };
  const rangeFor = (scrollTop: number): [number, number] => {
    if (viewportH === 0) return [0, Math.min(items.length - 1, 60)];
    const first = Math.max(0, rowAt(scrollTop) - OVERSCAN);
    const last = Math.min(items.length - 1, rowAt(scrollTop + viewportH) + OVERSCAN);
    return [first, last];
  };

  // Row the current find hit points at (guarded: matches can shrink a render
  // before the reset effect clamps findCur). -1 while the bar is closed so no
  // ring survives a close.
  const findCurRow =
    findOpen && findMatches.length > 0
      ? findMatches[Math.min(findCur, findMatches.length - 1)].row
      : -1;

  const renderColumn = (
    side: "left" | "right",
    ref: React.RefObject<HTMLDivElement | null>,
    range: [number, number],
    probe: string,
  ) => {
    const rows: React.ReactNode[] = [];
    for (let i = range[0]; i <= range[1]; i++) {
      const it = items[i];
      const top = offsets[i];
      const isHit = findMatchSet.has(i);
      const isCur = i === findCurRow;
      if (it.kind === "header") {
        rows.push(
          <div
            key={i}
            className={
              "sbs-hunk-header" +
              (side === "right" ? " sbs-hunk-header-blank" : "") +
              (isHit ? " sbs-find-hit" : "") +
              (isCur ? " sbs-find-cur" : "")
            }
            style={{ top, height: headerH }}
          >
            {side === "right" ? " " : it.text}
          </div>,
        );
      } else {
        rows.push(
          <Line
            key={i}
            side={side === "left" ? it.left : it.right}
            kind={side}
            top={top}
            height={lineH}
            highlighter={highlighter}
            lang={lang}
            dark={dark}
            findHit={isHit}
            findCurrent={isCur}
          />,
        );
      }
    }
    return (
      <div className="sbs-col" ref={ref}>
        <div className="sbs-col-inner" style={{ height: total }}>
          {/* In-flow, invisible — sizes the track to the widest line so the
              horizontal scrollbar stays put as the vertical window slides.
              Capped like the rendered lines: an uncapped multi-MB one-liner
              would lay out a multi-million-px run and stall the thread. */}
          <div className="sbs-line sbs-probe" aria-hidden="true">
            <span className="sbs-num" />
            <span className="sbs-sign" />
            <span className="sbs-text">
              {probe.length > LINE_RENDER_CAP + 64
                ? probe.slice(0, LINE_RENDER_CAP + 64)
                : probe}
            </span>
          </div>
          {rows}
        </div>
      </div>
    );
  };

  const rangeL = rangeFor(scrollTopL);
  const rangeR = locked ? rangeL : rangeFor(scrollTopR);

  const splitStyle = {
    "--sbs-l": `${split}fr`,
    "--sbs-r": `${1 - split}fr`,
  } as React.CSSProperties;

  return (
    <div className="sbs">
      {findOpen && (
        <div className="sbs-find" role="search">
          <input
            ref={findInputRef}
            className="sbs-find-input"
            type="text"
            value={findQuery}
            placeholder={i18n._(diffMessages.find)}
            spellCheck={false}
            aria-label={i18n._(diffMessages.findAria)}
            // No autoFocus: with a persisted-open bar this would steal focus
            // on every file switch. Ctrl+F focuses explicitly (rAF above).
            onChange={(e) => setFindQuery(e.target.value)}
            onKeyDown={(e) => {
              // IME (e.g. Hangul) composition commits/cancels with Enter/Esc
              // — those must never navigate or close the bar.
              if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) {
                return;
              }
              // Escape is handled by the capture-phase window listener above
              // (closes the bar everywhere, shields DiffApp's hide).
              if (e.key === "Enter") {
                e.preventDefault();
                gotoFindMatch(e.shiftKey ? -1 : 1);
              }
            }}
          />
          <span className="sbs-find-count" aria-live="polite">
            {findQuery.trim()
              ? findMatches.length
                ? `${Math.min(findCur, findMatches.length - 1) + 1}/${findMatches.length}`
                : "0"
              : ""}
          </span>
          <button
            type="button"
            className="sbs-find-btn"
            disabled={findMatches.length === 0}
            onClick={() => gotoFindMatch(-1)}
            title={i18n._(diffMessages.previousMatchTitle)}
            aria-label={i18n._(diffMessages.previousMatch)}
          >
            ↑
          </button>
          <button
            type="button"
            className="sbs-find-btn"
            disabled={findMatches.length === 0}
            onClick={() => gotoFindMatch(1)}
            title={i18n._(diffMessages.nextMatchTitle)}
            aria-label={i18n._(diffMessages.nextMatch)}
          >
            ↓
          </button>
          <button
            type="button"
            className="sbs-find-btn"
            onClick={() => setFindOpen(false)}
            title={i18n._(diffMessages.closeFindTitle)}
            aria-label={i18n._(diffMessages.closeFind)}
          >
            ✕
          </button>
        </div>
      )}
      <div className="sbs-cols" ref={colsRef} style={splitStyle}>
        {renderColumn("left", leftRef, rangeL, probeL)}
        <div
          className="sbs-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label={i18n._(diffMessages.resizeColumns)}
          title={i18n._(diffMessages.resizeColumnsTitle)}
          onPointerDown={onResizerDown}
          onPointerMove={onResizerMove}
          onPointerUp={finishDrag}
          onPointerCancel={finishDrag}
          onLostPointerCapture={finishDrag}
          onDoubleClick={() => {
            setSplit(0.5);
            saveSplit(0.5);
          }}
        />
        {renderColumn("right", rightRef, rangeR, probeR)}
      </div>
      {segments.length > 0 && (
        <DiffMinimap
          segments={segments}
          leftRef={leftRef}
          rightRef={rightRef}
          locked={locked}
        />
      )}
    </div>
  );
}

interface LineProps {
  side: DiffSide;
  kind: "left" | "right";
  top: number;
  height: number;
  highlighter: Highlighter | null;
  lang: ReturnType<typeof langForPath>;
  dark: boolean;
  /** Row matches the in-diff find query (tinted). */
  findHit: boolean;
  /** Row is the CURRENT find hit (stronger ring). */
  findCurrent: boolean;
}

// Memoized so a window slide (or any parent re-render) only touches rows that
// actually entered/left the viewport — unchanged rows skip re-highlighting.
const Line = memo(function Line({
  side,
  kind,
  top,
  height,
  highlighter,
  lang,
  dark,
  findHit,
  findCurrent,
}: LineProps) {
  const sign = side.type === "delete" ? "-" : side.type === "add" ? "+" : " ";

  const raw = side.text || " ";
  const overCap = raw.length > LINE_RENDER_CAP;
  const shown = overCap ? raw.slice(0, LINE_RENDER_CAP) : raw;

  // Highlighting is scroll-safe: a cache hit renders styled immediately; a
  // miss renders PLAIN TEXT this frame and upgrades asynchronously through
  // the budgeted queue (a viewport of synchronous codeToHtml calls used to
  // blow whole frames on a minimap drag). `undefined` = not computed yet.
  const wantHl = !!highlighter && !!lang && raw.length <= HL_LINE_CAP;
  const cached = wantHl
    ? highlightLineIfCached(raw, lang!, dark)
    : null;
  const [asyncHl, setAsyncHl] = useState<{
    key: string;
    html: string | null;
  } | null>(null);
  useEffect(() => {
    if (!wantHl || cached !== undefined) return;
    let live = true;
    const key = `${dark ? "d" : "l"}|${lang}|${raw}`;
    const cancel = highlightLineQueued(
      highlighter!,
      raw,
      lang!,
      dark,
      (html) => {
        if (live) setAsyncHl({ key, html });
      },
    );
    return () => {
      live = false;
      cancel();
    };
    // `cached === undefined` is the queue-needed signal; the rest identify
    // the line. eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wantHl, cached === undefined, highlighter, raw, lang, dark]);

  const asyncKey = `${dark ? "d" : "l"}|${lang}|${raw}`;
  const highlighted = wantHl
    ? cached !== undefined
      ? cached
      : asyncHl && asyncHl.key === asyncKey
        ? asyncHl.html
        : null
    : null;

  return (
    <div
      className={
        `sbs-line sbs-${kind} ${side.type ?? "blank"}` +
        (findHit ? " sbs-find-hit" : "") +
        (findCurrent ? " sbs-find-cur" : "")
      }
      data-line-num={side.lineNum ?? ""}
      data-side={kind}
      style={{ top, height }}
    >
      <span className="sbs-num">{side.lineNum ?? ""}</span>
      <span className="sbs-sign">{sign}</span>
      {highlighted ? (
        <span
          className="sbs-text sbs-text-shiki"
          // Shiki output is trusted — we built it locally from our diff text.
          dangerouslySetInnerHTML={{ __html: highlighted }}
        />
      ) : (
        <span className="sbs-text">
          {shown}
          {overCap && (
            <span className="sbs-line-truncated">
              {" "}
              … +{(raw.length - LINE_RENDER_CAP).toLocaleString()} chars (render
              truncated)
            </span>
          )}
        </span>
      )}
    </div>
  );
});
