import type { AuthorTally, Repo, WindowDays } from "../../types";

export type SearchScopePage = "summary" | "repositories" | "authors" | "time";

export interface SearchScopeChipProps {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  scopeLabel: string;
  repos: Repo[];
  repoCounts: Map<number, number>;
  repoCountsLoading: boolean;
  selectedRepoPath: string | null;
  selectedRepoPaths: string[] | "all";
  onSelectRepo: (path: string | null) => void;
  onSelectRepos: (paths: string[] | "all") => void;
  onSelectAllRepos: () => void;
  authors: AuthorTally[];
  authorsLoading: boolean;
  selectedAuthors: string[] | "all";
  onAuthorsChange: (authors: string[] | "all") => void;
  windowDays: WindowDays;
  onWindowDaysChange: (days: WindowDays) => void;
  singleMode: boolean;
}
