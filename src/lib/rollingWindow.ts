import type { WindowDays } from "../types";

const SECONDS_PER_DAY = 86_400;

export function filterRollingWindow<T extends { timestamp: number }>(
  items: T[],
  windowDays: WindowDays,
  nowSeconds: number,
): T[] {
  if (windowDays === "all") return items;
  const cutoff = Math.floor(nowSeconds) - windowDays * SECONDS_PER_DAY;
  const filtered = items.filter((item) => item.timestamp >= cutoff);
  // Most timer ticks and fresh backend snapshots remove nothing. Preserve
  // identity in that common case so React can also preserve the memoized DAG.
  return filtered.length === items.length ? items : filtered;
}

/** First integer second at which one currently-visible row must disappear. */
export function nextRollingWindowExpiry<T extends { timestamp: number }>(
  items: T[],
  windowDays: Exclude<WindowDays, "all">,
  nowSeconds: number,
): number | null {
  const now = Math.floor(nowSeconds);
  const span = windowDays * SECONDS_PER_DAY;
  let next: number | null = null;
  for (const item of items) {
    // Backend inclusion is timestamp >= now - span. At timestamp + span + 1
    // the integer-second cutoff moves past this row.
    const expiry = item.timestamp + span + 1;
    if (expiry <= now) continue;
    if (next == null || expiry < next) next = expiry;
  }
  return next;
}
