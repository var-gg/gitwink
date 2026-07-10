import { describe, expect, it } from "vitest";

import { isSearchResultCurrent } from "./searchResolution";

describe("isSearchResultCurrent", () => {
  it("rejects rows from the previous query during debounce", () => {
    expect(isSearchResultCurrent("fix race", "fix", "fix", "all", "all")).toBe(
      false,
    );
  });

  it("still rejects the old rows after Enter flushes the visible query", () => {
    expect(
      isSearchResultCurrent("fix race", "fix race", "fix", "all", "all"),
    ).toBe(false);
  });

  it("allows activation only after the applied snapshot catches up", () => {
    expect(
      isSearchResultCurrent(
        "fix race",
        "fix race",
        "fix race",
        "all",
        "all",
      ),
    ).toBe(true);
  });

  it("reuses a still-valid applied snapshot when typing returns to it", () => {
    // abc was applied, the user briefly typed abcd, then deleted d before
    // the debounce fired. No reload is needed and the cover must not stick.
    expect(isSearchResultCurrent("abc", "abc", "abc", "all", "all")).toBe(
      true,
    );
  });

  it("rejects rows resolved for the previous repo scope", () => {
    expect(isSearchResultCurrent("abc", "abc", "abc", "all", "repo:7")).toBe(
      false,
    );
  });
});
