import { describe, expect, it } from "vitest";

import {
  removeUnavailableFacetValue,
  toggleFacetValue,
  unavailableFacetValues,
} from "./facetSelection";

describe("contextual facet selections", () => {
  it("surfaces selected values that are absent from the current scope", () => {
    expect(
      unavailableFacetValues(["alice", "bob"], ["alice", "carol"]),
    ).toEqual(["carol"]);
    expect(unavailableFacetValues(["alice"], "all")).toEqual([]);
  });

  it("uses OR within a facet and collapses all available values", () => {
    expect(toggleFacetValue("all", "alice", ["alice", "bob"])).toEqual([
      "alice",
    ]);
    expect(
      toggleFacetValue(["alice"], "bob", ["alice", "bob"]),
    ).toBe("all");
  });

  it("does not collapse while an out-of-scope value is still selected", () => {
    expect(
      toggleFacetValue(["alice", "carol"], "bob", ["alice", "bob"]),
    ).toEqual(["alice", "carol", "bob"]);
  });

  it("clears the final unavailable value back to all", () => {
    expect(removeUnavailableFacetValue(["carol"], "carol")).toBe("all");
    expect(removeUnavailableFacetValue(["alice", "carol"], "carol")).toEqual([
      "alice",
    ]);
  });
});
