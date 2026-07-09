export type FacetSelection = string[] | "all";

/** Selected values that are absent from the facet's current contextual
 * option set. They stay visible as explicit zero/unavailable rows instead of
 * becoming a hidden filter. */
export function unavailableFacetValues(
  available: readonly string[],
  selected: FacetSelection,
): string[] {
  if (selected === "all") return [];
  const availableSet = new Set(available);
  return selected.filter((value) => !availableSet.has(value));
}

/** Toggle one facet value. Selecting every currently available value reduces
 * to the `all` sentinel only when no out-of-scope selection is also present. */
export function toggleFacetValue(
  selected: FacetSelection,
  value: string,
  available: readonly string[],
): FacetSelection {
  if (selected === "all") return [value];
  const nextSet = new Set(selected);
  if (nextSet.has(value)) nextSet.delete(value);
  else nextSet.add(value);
  const next = Array.from(nextSet);
  const selectsEveryAvailable =
    available.length > 0 &&
    next.length === available.length &&
    available.every((candidate) => nextSet.has(candidate));
  return selectsEveryAvailable ? "all" : next;
}

/** Clearing a selected value that no longer exists should remove the stale
 * restriction, not turn it into the explicit "select nothing" state. */
export function removeUnavailableFacetValue(
  selected: FacetSelection,
  value: string,
): FacetSelection {
  if (selected === "all") return "all";
  const next = selected.filter((candidate) => candidate !== value);
  return next.length === 0 ? "all" : next;
}
