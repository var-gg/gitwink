import { setupI18n } from "@lingui/core";
import { describe, expect, it } from "vitest";

import {
  authorSearchScopeLabel,
  combinedSearchScopeLabel,
  historySearchScopeLabel,
  repositorySearchScopeLabel,
} from "../components/panel/searchScopePresentation";
import { resolveSearchScope, searchNeedsCompleteHistory } from "./searchScope";
import { timelineSnapshotFilterKey } from "./useTimelineWindow";

const i18n = setupI18n({ locale: "en", messages: {} });
const repos = [
  { id: 7, path: "/work/alpha", name: "alpha", status: "active" as const },
  { id: 9, path: "/work/beta", name: "beta", status: "active" as const },
];

describe("combined commit-search scope", () => {
  it("inherits repository, author and time instead of silently widening", () => {
    expect(resolveSearchScope([7], ["alice"], 7)).toEqual({
      repoIds: [7],
      authors: ["alice"],
      windowDays: 7,
    });
    expect(resolveSearchScope(null, "all", "all")).toEqual({
      repoIds: null,
      authors: null,
      windowDays: null,
    });
  });

  it("starts full-history expansion only for an active all-time query", () => {
    expect(searchNeedsCompleteHistory(true, "fix", null)).toBe(true);
    expect(searchNeedsCompleteHistory(true, "fix", 7)).toBe(false);
    expect(searchNeedsCompleteHistory(true, "   ", null)).toBe(false);
    expect(searchNeedsCompleteHistory(false, "fix", null)).toBe(false);
  });

  it("makes every filter and query part of the snapshot identity", () => {
    const base = timelineSnapshotFilterKey([7], ["alice"], 7, "fix");
    expect(timelineSnapshotFilterKey([9], ["alice"], 7, "fix")).not.toBe(base);
    expect(timelineSnapshotFilterKey([7], ["bob"], 7, "fix")).not.toBe(base);
    expect(timelineSnapshotFilterKey([7], ["alice"], 30, "fix")).not.toBe(base);
    expect(timelineSnapshotFilterKey([7], ["alice"], 7, "race")).not.toBe(base);
  });

  it("describes the exact effective scope used by the result query", () => {
    const repository = repositorySearchScopeLabel(i18n, [7], repos);
    const author = authorSearchScopeLabel(i18n, ["alice"]);
    const history = historySearchScopeLabel(i18n, 7, false, false, null);
    expect(
      combinedSearchScopeLabel(i18n, repository, history, author, false),
    ).toBe("alpha · Last 7 days · alice");
  });
});
