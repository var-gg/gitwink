import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { chipRowH, useUiScale } from "../lib/settings";

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
  /** Called with the keys of the rows currently mounted (visible + overscan)
   * whenever that set changes — lets a caller lazily load data for just the
   * rows on screen (e.g. BranchChip's per-branch commit counts). */
  onVisibleKeys?: (keys: string[]) => void;
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
  onVisibleKeys,
}: Props) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [measuredViewportH, setMeasuredViewportH] = useState<number | null>(null);

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
  // Scale the maxHeight cap so a bigger UI gets a proportionally bigger
  // dropdown viewport, matching the scaled row heights.
  const effectiveMaxHeight = chipRowH(useUiScale(), maxHeight);
  const preferredViewportH = Math.min(total, effectiveMaxHeight);
  const viewportH = Math.min(
    preferredViewportH,
    measuredViewportH ?? preferredViewportH,
  );

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const update = () => setMeasuredViewportH(element.clientHeight);
    update();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(element);
    return () => observer?.disconnect();
  }, [preferredViewportH]);

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

  let first = 0;
  let last = -1;
  if (rows.length > 0) {
    first = Math.max(0, rowAt(scrollTop) - overscan);
    last = Math.min(rows.length - 1, rowAt(scrollTop + viewportH) + overscan);
  }

  const visible: ReactNode[] = [];
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

  // Report the mounted row keys so a caller can lazy-load per-row data. Held
  // in a ref so a changing callback identity doesn't re-fire the effect.
  const onVisibleKeysRef = useRef(onVisibleKeys);
  onVisibleKeysRef.current = onVisibleKeys;
  useEffect(() => {
    if (!onVisibleKeysRef.current) return;
    const keys: string[] = [];
    for (let i = first; i <= last; i++) keys.push(rows[i].key);
    onVisibleKeysRef.current(keys);
  }, [first, last, rows]);

  return (
    <div
      ref={scrollRef}
      className="chip-list chip-vlist"
      style={{ height: preferredViewportH }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div className="chip-vtrack" style={{ height: total }}>
        {visible}
      </div>
    </div>
  );
}
