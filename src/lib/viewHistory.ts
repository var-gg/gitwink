import type { WindowDays } from "../types";

/** A complete snapshot of the panel state that decides what the timeline
 * shows. Kept outside App so repo lifecycle changes can prune history with a
 * pure, regression-tested transformation. */
export interface ViewSnapshot {
  repoPath: string | null;
  repoPaths: string[] | "all";
  branches: string[] | "all";
  windowDays: WindowDays;
  authors: string[] | "all";
  fileHistory: { repoPath: string; filePath: string } | null;
  searchOpen: boolean;
  searchInput: string;
}

/** Remove references to an excluded canonical repo path without introducing
 * a navigation entry that could bring the repo back via Back/Forward. */
export function pruneRepoFromViewHistory(
  views: ViewSnapshot[],
  canonicalPath: string,
): ViewSnapshot[] {
  return views
    .filter(
      (view) =>
        view.repoPath !== canonicalPath &&
        view.fileHistory?.repoPath !== canonicalPath,
    )
    .map((view) => {
      if (!Array.isArray(view.repoPaths)) return view;
      const repoPaths = view.repoPaths.filter((path) => path !== canonicalPath);
      if (repoPaths.length === view.repoPaths.length) return view;
      return {
        ...view,
        repoPaths: repoPaths.length === 0 ? ("all" as const) : repoPaths,
      };
    });
}
