/** True only when the visible input, requested backend query, and the query
 * that actually produced the mounted result rows are identical. Comparing
 * only the first two re-enables a brief stale-row activation window while
 * the new IPC request is still resolving. */
export function isSearchResultCurrent(
  visibleInput: string,
  requestedQuery: string,
  appliedQuery: string | null,
  requestedScopeKey: string,
  appliedScopeKey: string | null,
): boolean {
  return (
    visibleInput === requestedQuery &&
    visibleInput === appliedQuery &&
    requestedScopeKey === appliedScopeKey
  );
}
