import type { WindowDays } from "../types";

/** The filter dimensions commit search inherits from the visible timeline.
 * Keeping this as a named value prevents a caller from silently widening one
 * dimension (the old search path hard-coded authors/time to null). */
export interface ResolvedSearchScope {
  repoIds: number[] | null;
  authors: string[] | null;
  windowDays: number | null;
}

export function resolveSearchScope(
  repoIds: number[] | null,
  selectedAuthors: string[] | "all",
  windowDays: WindowDays,
): ResolvedSearchScope {
  return {
    repoIds,
    authors: selectedAuthors === "all" ? null : selectedAuthors,
    windowDays: windowDays === "all" ? null : windowDays,
  };
}

/** Full-history Git expansion is valuable only when the active search can
 * actually return full-history rows. A 7-day search reuses the bounded cache
 * already maintained for that timeline instead of starting O(history) work. */
export function searchNeedsCompleteHistory(
  searchOpen: boolean,
  query: string,
  windowDays: number | null,
): boolean {
  return searchOpen && query.trim().length > 0 && windowDays == null;
}
