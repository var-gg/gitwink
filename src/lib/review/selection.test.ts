import { expect, it } from "vitest";
import { initialReviewSelection } from "./selection";
import type { ReviewFile } from "./ipc";

it("never opens or preselects untracked content when it sorts before tracked changes", () => {
  const files = [{ id: "untracked:a", group: "untracked" }, { id: "unstaged:z", group: "unstaged" }] as ReviewFile[];
  expect(initialReviewSelection(files)).toEqual({ active: "unstaged:z", selected: new Set(["unstaged:z"]) });
  expect(initialReviewSelection(files.slice(0, 1))).toEqual({ active: null, selected: new Set() });
});
