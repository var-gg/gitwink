/** Shared ids for the commit-search combobox/listbox relationship. */
export const SEARCH_RESULTS_LIST_ID = "commit-search-results";

/** The global row index is unique inside one listbox and remains stable while
 * its backing virtual window is paged. */
export function searchResultOptionId(globalIndex: number): string {
  return `commit-search-option-${globalIndex}`;
}
