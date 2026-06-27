import {
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { Highlighter } from "shiki";

import { parseDiff, type DiffSide } from "../lib/diff";
import {
  flattenDiff,
  longestLines,
} from "../lib/diffView";
import {
  getHighlighter,
  highlightLineCached,
  langForPath,
} from "../lib/highlight";
import { sbsHeaderH, sbsLineH, useUiScale } from "../lib/settings";
import { DiffMinimap } from "./DiffMinimap";

interface Props {
  text: string;
  /** File path so we can detect the language for Shiki. Optional — falls
   * back to plain monospace when missing or unknown. */
  filePath?: string;
  /** When true, the two columns scroll vertically as one (default). When
   * false they scroll independently — the overview rail then shows both
   * viewports and offers a re-align. */
  locked: boolean;
}

/** Rows mounted above & below the viewport so a fast flick never shows a gap
 * before the next window resolves. */
const OVERSCAN = 8;

function isDarkScheme(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-color-scheme: dark)").matches
  );
}

// Persisted old/new split — fraction of width given to the left (old)
// column. Clamped so neither side can be dragged to nothing.
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

export function SideBySideDiff({ text, filePath, locked }: Props) {
  const scale = useUiScale();
  const lineH = sbsLineH(scale);
  const headerH = sbsHeaderH(scale);

  // Parse → flat items + overview segments. Memoized so dragging the splitter
  // (which we keep off React entirely, below) or a scroll never reparses.
  const { items, segments } = useMemo(
    () => flattenDiff(parseDiff(text).hunks),
    [text],
  );

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
  }, []);

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

  // Open every new file at the top — the persisted webview means the old
  // scroll would otherwise carry over to an unrelated file.
  useLayoutEffect(() => {
    if (leftRef.current) leftRef.current.scrollTop = 0;
    if (rightRef.current) rightRef.current.scrollTop = 0;
    setScrollTopL(0);
    setScrollTopR(0);
  }, [text]);

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
  }, [locked]);

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

  if (items.length === 0) {
    return <div className="sbs-empty">No textual diff.</div>;
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
      if (it.kind === "header") {
        rows.push(
          <div
            key={i}
            className={
              "sbs-hunk-header" + (side === "right" ? " sbs-hunk-header-blank" : "")
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
          />,
        );
      }
    }
    return (
      <div className="sbs-col" ref={ref}>
        <div className="sbs-col-inner" style={{ height: total }}>
          {/* In-flow, invisible — sizes the track to the widest line so the
              horizontal scrollbar stays put as the vertical window slides. */}
          <div className="sbs-line sbs-probe" aria-hidden="true">
            <span className="sbs-num" />
            <span className="sbs-sign" />
            <span className="sbs-text">{probe}</span>
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
      <div className="sbs-cols" ref={colsRef} style={splitStyle}>
        {renderColumn("left", leftRef, rangeL, probeL)}
        <div
          className="sbs-resizer"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize old/new columns"
          title="Drag to resize · double-click to reset"
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
}: LineProps) {
  const sign = side.type === "delete" ? "-" : side.type === "add" ? "+" : " ";

  const highlighted =
    highlighter && lang
      ? highlightLineCached(highlighter, side.text || " ", lang, dark)
      : null;

  return (
    <div
      className={`sbs-line sbs-${kind} ${side.type ?? "blank"}`}
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
        <span className="sbs-text">{side.text || " "}</span>
      )}
    </div>
  );
});
