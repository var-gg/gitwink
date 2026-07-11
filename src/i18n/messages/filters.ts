import { msg } from "@lingui/core/macro";

export const filterMessages = {
  clearFilter: msg({ id: "filters.clear", message: "Clear filter" }),
  contextMenu: msg({ id: "common.contextMenu", message: "Context menu" }),

  allReposCount: msg({
    id: "filters.repos.allCount",
    message: "All repositories ({count})",
  }),
  repoCount: msg({
    id: "filters.repos.count",
    message: "{count, plural, one {# repository} other {# repositories}}",
  }),
  backToAllRepos: msg({
    id: "filters.repos.backToAll",
    message: "Back to all repositories",
  }),
  clearRepo: msg({
    id: "filters.repos.clear",
    message: "Clear repository filter",
  }),
  multiRepoOnly: msg({
    id: "filters.repos.multiRepoOnly",
    message: "Multi-select is available in all-repositories mode",
  }),
  pinned: msg({ id: "filters.repos.pinned", message: "📌 Pinned" }),
  allRepos: msg({ id: "filters.repos.all", message: "All repositories" }),
  noReposMatch: msg({
    id: "filters.repos.noMatch",
    message: "No repositories match.",
  }),
  searchRepos: msg({
    id: "filters.repos.searchPlaceholder",
    message: "Search repositories…",
  }),
  searchReposAria: msg({
    id: "filters.repos.searchAria",
    message: "Search repositories",
  }),
  excludeFailed: msg({
    id: "filters.repos.excludeFailed",
    message: "Couldn't exclude repository: {error}",
  }),
  exclude: msg({
    id: "filters.repos.exclude",
    message: "Exclude from gitwink",
  }),
  excludeDescription: msg({
    id: "filters.repos.excludeDescription",
    message: "Keeps the folder and preferences. Restore later in Settings.",
  }),
  filterTimelineTo: msg({
    id: "filters.repos.filterTimelineTo",
    message: "Filter timeline to {repo}",
  }),
  addToMultiRepo: msg({
    id: "filters.repos.addToMulti",
    message: "Add to multi-repository filter",
  }),
  missingAria: msg({
    id: "filters.repos.missingAria",
    message: "{repo}, repository missing at {path}",
  }),
  openAria: msg({
    id: "filters.repos.openAria",
    message: "Open {repo} at {path}",
  }),
  missingTitle: msg({
    id: "filters.repos.missingTitle",
    message:
      "{path} — moved or deleted on disk. Drop the new path on the panel to relink, or select exclude.",
  }),
  missing: msg({ id: "filters.repos.missing", message: "missing" }),
  updatingCommitCounts: msg({
    id: "filters.repos.updatingCommitCounts",
    message: "Updating commits for the current author and time filters",
  }),
  matchingCommitCounts: msg({
    id: "filters.repos.matchingCommitCounts",
    message: "Commits matching the current author and time filters",
  }),
  updatingCommitCountAria: msg({
    id: "filters.repos.updatingCommitCountAria",
    message: "Updating commit count",
  }),
  excludeTitle: msg({
    id: "filters.repos.excludeTitle",
    message: "Exclude from gitwink (restore later in Settings)",
  }),
  excludeAria: msg({
    id: "filters.repos.excludeAria",
    message: "Exclude {repo} from gitwink",
  }),
  pin: msg({ id: "filters.repos.pin", message: "Pin" }),
  unpin: msg({ id: "filters.repos.unpin", message: "Unpin" }),
  pinAria: msg({ id: "filters.repos.pinAria", message: "Pin {repo}" }),
  unpinAria: msg({ id: "filters.repos.unpinAria", message: "Unpin {repo}" }),

  allAuthors: msg({ id: "filters.authors.all", message: "All authors" }),
  noAuthors: msg({ id: "filters.authors.none", message: "No authors" }),
  authorCount: msg({
    id: "filters.authors.count",
    message: "{count, plural, one {# author} other {# authors}}",
  }),
  authorCountUnavailable: msg({
    id: "filters.authors.countUnavailable",
    message:
      "{count, plural, one {# author} other {# authors}} · {unavailable} unavailable",
  }),
  updatingAuthors: msg({
    id: "filters.authors.updating",
    message: "Updating authors…",
  }),
  unavailableAuthorsTitle: msg({
    id: "filters.authors.unavailableTitle",
    message:
      "Selected authors with no commits in the current repository, branch, and time scope",
  }),
  unavailableAuthors: msg({
    id: "filters.authors.unavailable",
    message: "Selected · unavailable in this scope",
  }),
  unavailableAuthorRemove: msg({
    id: "filters.authors.unavailableRemove",
    message: "{author} has no commits in the current scope. Select to remove this filter.",
  }),
  showOnlyAuthor: msg({
    id: "filters.authors.showOnly",
    message: "Show only {author}",
  }),
  only: msg({ id: "filters.authors.only", message: "only" }),
  authorCountTitle: msg({
    id: "filters.authors.countTitle",
    message: "Commits in the current repository and time scope",
  }),
  noAuthorsMatch: msg({
    id: "filters.authors.noMatch",
    message: "No authors match.",
  }),
  searchAuthors: msg({
    id: "filters.authors.searchPlaceholder",
    message: "Search authors…",
  }),
  searchAuthorsAria: msg({
    id: "filters.authors.searchAria",
    message: "Search authors",
  }),
  sortAuthors: msg({ id: "filters.authors.sort", message: "Sort authors" }),
  sort: msg({ id: "filters.sort", message: "Sort" }),
  sortCount: msg({ id: "filters.authors.sortCount", message: "Count" }),
  sortCountTitle: msg({
    id: "filters.authors.sortCountTitle",
    message: "Most commits first",
  }),
  sortName: msg({ id: "filters.authors.sortName", message: "A–Z" }),
  sortNameTitle: msg({
    id: "filters.authors.sortNameTitle",
    message: "Alphabetical",
  }),
  sortRecent: msg({ id: "filters.authors.sortRecent", message: "Recent" }),
  sortRecentTitle: msg({
    id: "filters.authors.sortRecentTitle",
    message: "Most recent activity first",
  }),

  loadingBranches: msg({
    id: "filters.branches.loading",
    message: "Loading branches…",
  }),
  allBranchesCount: msg({
    id: "filters.branches.allCount",
    message: "All branches ({count})",
  }),
  allBranches: msg({ id: "filters.branches.all", message: "All branches" }),
  noBranches: msg({ id: "filters.branches.none", message: "No branches" }),
  branchCount: msg({
    id: "filters.branches.count",
    message: "{count, plural, one {# branch} other {# branches}}",
  }),
  branchUnavailable: msg({
    id: "filters.branches.unavailableSuffix",
    message: "{branch} · unavailable",
  }),
  branchCountUnavailable: msg({
    id: "filters.branches.countUnavailable",
    message:
      "{count, plural, one {# branch} other {# branches}} · {unavailable} unavailable",
  }),
  lastActivity: msg({
    id: "filters.branches.lastActivity",
    message: "Last activity: {date}",
  }),
  countUnavailable: msg({
    id: "filters.branches.countUnavailableTitle",
    message:
      "Count unavailable after {count} retries. Close and reopen this list to retry.",
  }),
  reachableCommits: msg({
    id: "filters.branches.reachableCommits",
    message: "Reachable commits across all time (independent of the time filter)",
  }),
  unavailableRefsTitle: msg({
    id: "filters.branches.unavailableTitle",
    message: "Selected refs that no longer exist in this repository",
  }),
  unavailableRefs: msg({
    id: "filters.branches.unavailable",
    message: "Selected · unavailable refs",
  }),
  unavailableRefRemove: msg({
    id: "filters.branches.unavailableRemove",
    message: "{ref} no longer exists. Select to remove this filter.",
  }),
  missingRef: msg({ id: "filters.branches.missing", message: "missing" }),
  localBranches: msg({ id: "filters.branches.local", message: "Local" }),
  remoteTracking: msg({
    id: "filters.branches.remoteTracking",
    message: "Remote tracking",
  }),
  remoteTrackingTitle: msg({
    id: "filters.branches.remoteTrackingTitle",
    message:
      "Remote-tracking refs are read locally—refreshed by your IDE or CLI, or by gitwink auto-fetch when the panel opens (Settings → Auto-fetch).",
  }),
  noBranchesMatch: msg({
    id: "filters.branches.noMatch",
    message: "No branches match.",
  }),
  searchBranches: msg({
    id: "filters.branches.searchPlaceholder",
    message: "Search branches…",
  }),
  searchBranchesAria: msg({
    id: "filters.branches.searchAria",
    message: "Search branches",
  }),

  searchCommitsPlaceholder: msg({
    id: "filters.search.placeholder",
    message: "Search commits—message, author, SHA prefix…",
  }),
  searchCommitsAria: msg({
    id: "filters.search.aria",
    message: "Search commits",
  }),
  clearSearch: msg({ id: "filters.search.clear", message: "Clear search" }),
  searchMatchCount: msg({
    id: "filters.search.matchCount",
    message: "{count, plural, one {# match} other {# matches}}",
  }),
  searchScope: msg({
    id: "filters.search.scope",
    message: "Search scope: {scope}",
  }),
  searchKeyboardHint: msg({
    id: "filters.search.keyboardHint",
    message:
      "↑↓ select · Enter jumps to history · click a row to preview · Esc closes",
  }),
  jumpToHistory: msg({
    id: "filters.search.jumpToHistory",
    message: "jump to history",
  }),

  last24Hours: msg({ id: "filters.time.last24Hours", message: "Last 24 hours" }),
  lastDays: msg({ id: "filters.time.lastDays", message: "Last {count} days" }),
  allTime: msg({ id: "filters.time.all", message: "All time" }),
} as const;
