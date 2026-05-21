import {
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

/** One row in a virtualised chip dropdown. `height` is the row's fixed
 * pixel height — rows of a kind are uniform, so the caller knows it up
 * front — and `render` produces the row content (called only for rows
 * inside the viewport). Keep `key` stable for a given logical row so
 * React recycles DOM nodes as the window slides. */
export interface VirtualChipRow {
  key: string;
  height: number;
  render: () => ReactNode;
}

interface Props {
  /** Flattened, ordered rows — section headers and items interleaved. */
  rows: VirtualChipRow[];
  /** Viewport cap in px; the list shrinks to fit when the rows total
   * less than this. Keep it under (dropdown max-height − search box). */
  maxHeight?: number;
  /** Rows of overscan mounted above & below the viewport. */
  overscan?: number;
  /** Changing this resets the scroll to the top — pass the search query
   * so filtering jumps back to the first match. */
  resetKey?: string;
}

/** Fixed-row-height DOM virtualisation for the filter-chip dropdowns.
 * Only the rows intersecting the viewport (plus overscan) are mounted, so
 * a 10 000-repo dropdown holds ~15 DOM rows, not 10 000 — opening it stays
 * a constant-time operation no matter how many repos were scanned. */
export function VirtualChipList({
  rows,
  maxHeight = 280,
  overscan = 4,
  resetKey,
}: Props) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);

  // Prefix sum of row heights: offsets[i] is the pixel top of row i and
  // offsets[rows.length] the total content height. Plain numbers, no DOM,
  // so an O(n) rebuild is cheap even for tens of thousands of rows.
  const offsets = useMemo(() => {
    const arr = new Array<number>(rows.length + 1);
    arr[0] = 0;
    for (let i = 0; i < rows.length; i++) arr[i + 1] = arr[i] + rows[i].height;
    return arr;
  }, [rows]);
  const total = offsets[rows.length];
  const viewportH = Math.min(total, maxHeight);

  // Snap back to the top whenever the filter query changes. useLayoutEffect
  // so the reset lands before paint — no flash of the old scroll position.
  useLayoutEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    setScrollTop(0);
  }, [resetKey]);

  // Largest row index whose top edge is <= y. Binary search keeps the
  // visible-range math O(log n) per scroll tick.
  const rowAt = (y: number) => {
    let lo = 0;
    let hi = rows.length - 1;
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

  const visible: ReactNode[] = [];
  if (rows.length > 0) {
    const first = Math.max(0, rowAt(scrollTop) - overscan);
    const last = Math.min(
      rows.length - 1,
      rowAt(scrollTop + viewportH) + overscan,
    );
    for (let i = first; i <= last; i++) {
      const row = rows[i];
      visible.push(
        <div
          key={row.key}
          className="chip-vrow"
          style={{ top: offsets[i], height: row.height }}
        >
          {row.render()}
        </div>,
      );
    }
  }

  return (
    <div
      ref={scrollRef}
      className="chip-list chip-vlist"
      style={{ height: viewportH }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div className="chip-vtrack" style={{ height: total }}>
        {visible}
      </div>
    </div>
  );
}
