import { describe, expect, it } from "vitest";

import { pruneRepoFromViewHistory, type ViewSnapshot } from "./viewHistory";

function view(overrides: Partial<ViewSnapshot> = {}): ViewSnapshot {
  return {
    repoPath: null,
    repoPaths: "all",
    branches: "all",
    windowDays: 7,
    authors: "all",
    fileHistory: null,
    searchOpen: false,
    searchInput: "",
    ...overrides,
  };
}

describe("pruneRepoFromViewHistory", () => {
  it("drops single-repo and file-history snapshots for the excluded repo", () => {
    const result = pruneRepoFromViewHistory(
      [
        view({ repoPath: "/repos/remove" }),
        view({
          fileHistory: { repoPath: "/repos/remove", filePath: "README.md" },
        }),
        view({ repoPath: "/repos/keep" }),
      ],
      "/repos/remove",
    );

    expect(result).toEqual([view({ repoPath: "/repos/keep" })]);
  });

  it("widens an emptied multi-repo snapshot and preserves surviving paths", () => {
    const result = pruneRepoFromViewHistory(
      [
        view({ repoPaths: ["/repos/remove"] }),
        view({ repoPaths: ["/repos/keep", "/repos/remove"] }),
      ],
      "/repos/remove",
    );

    expect(result[0].repoPaths).toBe("all");
    expect(result[1].repoPaths).toEqual(["/repos/keep"]);
  });

  it("does not collapse case-distinct canonical paths", () => {
    const result = pruneRepoFromViewHistory(
      [view({ repoPaths: ["/repos/Foo", "/repos/foo"] })],
      "/repos/Foo",
    );

    expect(result[0].repoPaths).toEqual(["/repos/foo"]);
  });
});
