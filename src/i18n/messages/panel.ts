import { msg } from "@lingui/core/macro";

export const panelMessages = {
  retry: msg({ id: "common.retry", message: "Retry" }),
  gotIt: msg({ id: "common.gotIt", message: "Got it" }),
  settings: msg({ id: "settings.title", message: "Settings" }),
  settingsMenu: msg({ id: "settings.openMenu", message: "Settings…" }),
  addRepoFailed: msg({
    id: "panel.addRepo.failed",
    message: "Failed to add repository",
  }),
  addRepoDialogTitle: msg({
    id: "panel.addRepo.dialogTitle",
    message: "Add a Git repository",
  }),
  selectedRepoScopeLoading: msg({
    id: "panel.search.selectedRepoScopeLoading",
    message: "selected repository scope (loading)",
  }),
  repoCount: msg({
    id: "panel.search.repoCount",
    message: "{count, plural, one {# repository} other {# repositories}}",
  }),
  searching: msg({ id: "panel.search.searching", message: "Searching…" }),
  jumpToCommit: msg({
    id: "timeline.jumpToCommit",
    message: "Jump to this commit in its repository history (Enter)",
  }),
  taggedCommit: msg({ id: "timeline.marker.tagged", message: "Tagged commit" }),
  mergeCommit: msg({ id: "timeline.marker.merge", message: "Merge commit" }),
  commit: msg({ id: "timeline.marker.commit", message: "Commit" }),
  noSummary: msg({ id: "timeline.noSummary", message: "(no summary)" }),
  newMarker: msg({ id: "timeline.marker.new", message: "{marker} (new)" }),
  repoFilterTitle: msg({
    id: "timeline.repoFilterTitle",
    message: "{path} (select to filter)",
  }),
  remoteTip: msg({
    id: "timeline.remoteTip",
    message:
      "Remote-tracking ref at this commit (read-only, from your last Git fetch).",
  }),
  remoteTips: msg({
    id: "timeline.remoteTips",
    message:
      "Remote-tracking refs at this commit (read-only, from your last Git fetch). {count} more.",
  }),
  copiedAi: msg({
    id: "timeline.copy.copiedAi",
    message: "Copied as AI context ✓",
  }),
  copyFailedTry: msg({
    id: "timeline.copy.failedTry",
    message: "Copy failed—try again",
  }),
  copied: msg({ id: "timeline.copy.copied", message: "Copied ✓" }),
  copyFailed: msg({ id: "timeline.copy.failed", message: "Copy failed" }),
  copyAi: msg({
    id: "timeline.copy.ai",
    message: "Copy as AI context",
  }),
  copyAiTitle: msg({
    id: "timeline.copy.aiTitle",
    message: "Copy as AI context (c)",
  }),
  dismiss: msg({ id: "common.dismiss", message: "Dismiss" }),
  anchorOlder: msg({
    id: "timeline.anchor.older",
    message: "Commit {hash} is older than the {count} commits loaded here.",
  }),
  jumpLatest: msg({
    id: "timeline.new.jumpLatest",
    message: "Jump to the latest commits",
  }),
  newCommits: msg({
    id: "timeline.new.count",
    message: "↑ {count, plural, one {# new commit} other {# new commits}}",
  }),
  loadFailed: msg({
    id: "timeline.loadFailed",
    message: "Couldn't load the timeline.",
  }),
  noMatchesScope: msg({
    id: "timeline.search.noMatchesScope",
    message: "No matches in {scope}.",
  }),
  noMatchesScanned: msg({
    id: "timeline.search.noMatchesScanned",
    message: "No matches in scanned history.",
  }),
  searchAllRepos: msg({
    id: "timeline.search.allRepos",
    message: "Search all repositories",
  }),
  searchAllReposTitle: msg({
    id: "timeline.search.allReposTitle",
    message: "Re-run this search across every repository",
  }),
  searchAllAuthors: msg({
    id: "timeline.search.allAuthors",
    message: "Search all authors",
  }),
  searchAllAuthorsTitle: msg({
    id: "timeline.search.allAuthorsTitle",
    message: "Re-run this search without the author filter",
  }),
  searchAllHistory: msg({
    id: "timeline.search.allHistory",
    message: "Search all history",
  }),
  searchAllHistoryTitle: msg({
    id: "timeline.search.allHistoryTitle",
    message: "Re-run this search without the time filter",
  }),
  commitSearchResults: msg({
    id: "timeline.search.results",
    message: "Commit search results",
  }),
  facetsError: msg({
    id: "panel.notices.facetsError",
    message: "Filter counts couldn't be refreshed.",
  }),
  historyPaused: msg({
    id: "panel.notices.historyPaused",
    message:
      "Complete-history indexing paused. Results stay limited to commits already indexed.",
  }),
  historyPreparing: msg({
    id: "panel.notices.historyPreparing",
    message: "Preparing the complete history index…",
  }),
  historyAvailable: msg({
    id: "panel.notices.historyAvailable",
    message:
      "Complete history is available for {published}/{total} repositories{failed}{offline}.",
  }),
  historyFailed: msg({
    id: "panel.notices.historyFailed",
    message: "; {count} could not be read",
  }),
  historyOffline: msg({
    id: "panel.notices.historyOffline",
    message: "; {count} are offline",
  }),
  historyIndexing: msg({
    id: "panel.notices.historyIndexing",
    message:
      "Indexing complete history · {published}/{total} repositories available · {commits} commits scanned.",
  }),
  autoFetchDisclosure: msg({
    id: "panel.notices.autoFetchDisclosure",
    message:
      "gitwink now fetches the open repository from origin when you view it, so a teammate's pushed commits appear. It updates only the remote-tracking mirror—it never merges, pushes, or changes your repository.",
  }),
  controlOpened: msg({
    id: "panel.control.opened",
    message: "Opened by {source}",
  }),
  controlFailed: msg({
    id: "panel.control.failed",
    message: "Request from {source} failed",
  }),
  controlDismiss: msg({
    id: "panel.control.dismiss",
    message: "Dismiss external request status",
  }),
  firstRunTip: msg({
    id: "panel.firstRun.tip",
    message:
      "search · click a commit to expand · the history button on a file opens its history · right-click to copy for AI",
  }),
  dismissTip: msg({ id: "panel.firstRun.dismiss", message: "Dismiss tip" }),
  dropTitle: msg({
    id: "panel.empty.dropTitle",
    message: "Drop a repository folder here",
  }),
  pastePath: msg({
    id: "panel.empty.pastePath",
    message: "or copy its path from your file manager",
  }),
  browseFolder: msg({
    id: "panel.empty.browseFolder",
    message: "Browse for a folder…",
  }),
  scanningRepos: msg({
    id: "panel.empty.scanningRepos",
    message: "Scanning for repositories…",
  }),
  addRepo: msg({ id: "panel.footer.addRepo", message: "+ Add repo…" }),
  or: msg({ id: "panel.footer.or", message: "or" }),
  pasteShort: msg({ id: "panel.footer.pasteShort", message: "Paste path" }),
  pasteBusy: msg({ id: "panel.footer.pasteBusy", message: "Pasting…" }),
  pasteTitle: msg({
    id: "panel.footer.pasteTitle",
    message: "Paste repository paths from the clipboard ({shortcut})",
  }),
  pasteNoPath: msg({
    id: "panel.footer.pasteNoPath",
    message: "The clipboard does not contain an absolute repository path.",
  }),
  pasteReadFailed: msg({
    id: "panel.footer.pasteReadFailed",
    message: "Couldn't read the clipboard.",
  }),
  loadingFileHistory: msg({
    id: "panel.single.loadingFileHistory",
    message: "Scanning file history…",
  }),
  loadingCommits: msg({
    id: "panel.single.loadingCommits",
    message: "Loading commits…",
  }),
  repoUnavailable: msg({
    id: "panel.single.repoUnavailable",
    message:
      "Couldn't open this repository—it may have moved or be on a disconnected drive.",
  }),
  backToAllRepos: msg({
    id: "panel.single.backToAllRepos",
    message: "Back to all repositories",
  }),
  fileHistoryUnavailable: msg({
    id: "panel.single.fileHistoryUnavailable",
    message:
      "Couldn't read this file's history—the repository may be unavailable or changing on disk.",
  }),
  exitFileHistory: msg({
    id: "panel.single.exitFileHistory",
    message: "Exit file history",
  }),
  fileEmpty: msg({
    id: "panel.single.fileEmpty",
    message: "No history found for {file} in this repository.",
  }),
  fileEmptyScanLimit: msg({
    id: "panel.single.fileEmptyScanLimit",
    message:
      "No change found for {file} in readable history. Older commits remain beyond the {count}-commit safety limit.",
  }),
  fileEmptyIncomplete: msg({
    id: "panel.single.fileEmptyIncomplete",
    message:
      "No change found for {file} in readable history. Some Git objects couldn't be read, so the result is incomplete.",
  }),
  fileEmptyShallow: msg({
    id: "panel.single.fileEmptyShallow",
    message:
      "No history found for {file} in this shallow clone; earlier history isn't present locally.",
  }),
  renamesNotFollowed: msg({
    id: "panel.single.renamesNotFollowed",
    message: "Renames aren't followed yet.",
  }),
  noMatchLimited: msg({
    id: "panel.single.noMatchLimited",
    message:
      "No matching commits in the latest {limit} loaded for this scope. Older commits haven't been checked.",
  }),
  noMatchIncomplete: msg({
    id: "panel.single.noMatchIncomplete",
    message: "No matching commits in the history that could be read completely.",
  }),
  noMatchShallow: msg({
    id: "panel.single.noMatchShallow",
    message: "No matching commits in the history available in this shallow clone.",
  }),
  noCommits24h: msg({
    id: "panel.single.noCommits24h",
    message: "No commits in the last 24 hours.",
  }),
  noCommitsDays: msg({
    id: "panel.single.noCommitsDays",
    message: "No commits in the last {days} days.",
  }),
  noCommits: msg({ id: "panel.single.noCommits", message: "No commits match." }),
  authorsFiltered: msg({
    id: "panel.single.authorsFiltered",
    message: " Filtered to {count, plural, one {# author} other {# authors}}.",
  }),
  branchesFiltered: msg({
    id: "panel.single.branchesFiltered",
    message: " Filtered to {count, plural, one {# branch} other {# branches}}.",
  }),
  showAllTime: msg({ id: "panel.single.showAllTime", message: "Show all time" }),
  clearAuthorFilter: msg({
    id: "panel.single.clearAuthorFilter",
    message: "Clear author filter",
  }),
  allBranches: msg({ id: "panel.single.allBranches", message: "All branches" }),
  searchCommits: msg({ id: "panel.search.commits", message: "Search commits" }),
  searchCommitsTitle: msg({
    id: "panel.search.commitsTitle",
    message: "Search commits—message, author, SHA (/)",
  }),
  coverageLimited: msg({
    id: "panel.single.coverageLimited",
    message:
      "Showing the latest {limit} commits in this scope. Older commits haven't been loaded.",
  }),
  coverageAuthorLimited: msg({
    id: "panel.single.coverageAuthorLimited",
    message:
      "Author results are limited to the latest {limit} loaded commits. Older author matches haven't been checked.",
  }),
  coverageIncomplete: msg({
    id: "panel.single.coverageIncomplete",
    message:
      "Some refs or Git objects couldn't be read, so this repository history may be incomplete.",
  }),
  coverageShallow: msg({
    id: "panel.single.coverageShallow",
    message: "This is a shallow clone. History before its local boundary isn't available.",
  }),
  fileCoverageMatches: msg({
    id: "panel.single.fileCoverageMatches",
    message: "Showing the latest {count} file changes. Older changes also exist.",
  }),
  fileCoverageScanned: msg({
    id: "panel.single.fileCoverageScanned",
    message: "Scanned {count} commits. Older history remains outside the safety limit.",
  }),
  fileCoverageShallow: msg({
    id: "panel.single.fileCoverageShallow",
    message:
      "Showing all file history available in this shallow clone; earlier history isn't present locally.",
  }),
  fileCoverageIncomplete: msg({
    id: "panel.single.fileCoverageIncomplete",
    message: "Some Git objects couldn't be read, so this file history may be incomplete.",
  }),
  allReposScope: msg({ id: "panel.scope.allRepos", message: "all repositories" }),
  typeToSearch: msg({
    id: "panel.search.typeToSearch",
    message: "Type to search {scope}",
  }),
  searchScope: msg({
    id: "panel.search.scope",
    message: "{scope} · {history} · {authors}{branches}",
  }),
  allBranchesSuffix: msg({
    id: "panel.search.allBranchesSuffix",
    message: " · all branches",
  }),
  historyAll: msg({ id: "panel.scope.historyAll", message: "all history" }),
  historyIndexed: msg({
    id: "panel.scope.historyIndexed",
    message: "currently indexed history",
  }),
  historyPreparingShort: msg({
    id: "panel.scope.historyPreparing",
    message: "preparing history index",
  }),
  historyIndexedCount: msg({
    id: "panel.scope.historyIndexedCount",
    message: "indexed history ({published}/{total} repositories)",
  }),
  historyIndexingCount: msg({
    id: "panel.scope.historyIndexingCount",
    message: "indexing history ({published}/{total} repositories)",
  }),
  fileHistoryUnavailableShort: msg({
    id: "panel.fileHistory.unavailable",
    message: "history unavailable",
  }),
  scanningShort: msg({ id: "panel.fileHistory.scanning", message: "scanning…" }),
  latestChanges: msg({
    id: "panel.fileHistory.latestChanges",
    message: "latest {count} changes",
  }),
  commitsScannedShort: msg({
    id: "panel.fileHistory.commitsScanned",
    message: "{count} commits scanned",
  }),
  partialHistory: msg({ id: "panel.fileHistory.partial", message: "partial history" }),
  cloneHistory: msg({
    id: "panel.fileHistory.cloneHistory",
    message: "available clone history",
  }),
  fileHistoryTitle: msg({
    id: "panel.fileHistory.title",
    message: "History of {path}: {scope}—click to exit (Esc)",
  }),
  fileHistoryRefs: msg({
    id: "panel.fileHistory.refs",
    message:
      "{scope}; {count} remote-tracking refs included; renames are not followed yet",
  }),
  updateAvailableTitle: msg({
    id: "panel.chrome.updateAvailableTitle",
    message: "Update available—v{version} · click to update",
  }),
  updateAria: msg({
    id: "panel.chrome.updateAria",
    message: "Update gitwink to version {version}",
  }),
  viewHistory: msg({ id: "panel.chrome.viewHistory", message: "View history" }),
  back: msg({ id: "panel.chrome.back", message: "Back" }),
  backTitle: msg({ id: "panel.chrome.backTitle", message: "Back (Alt+←)" }),
  forward: msg({ id: "panel.chrome.forward", message: "Forward" }),
  forwardTitle: msg({
    id: "panel.chrome.forwardTitle",
    message: "Forward (Alt+→)",
  }),
  scanning: msg({ id: "panel.chrome.scanning", message: "Scanning…" }),
  closeSearch: msg({ id: "panel.search.close", message: "Close commit search" }),
  pinTitle: msg({
    id: "panel.chrome.pinTitle",
    message:
      "Pin—keep open while clicking elsewhere; not always-on-top. Summon via tray or hotkey.",
  }),
  unpinTitle: msg({
    id: "panel.chrome.unpinTitle",
    message: "Unpin—return to glance mode (auto-hides on blur, always-on-top)",
  }),
  pinPanel: msg({ id: "panel.chrome.pinPanel", message: "Pin panel" }),
  unpinPanel: msg({ id: "panel.chrome.unpinPanel", message: "Unpin panel" }),
  maximizeTitle: msg({
    id: "panel.chrome.maximizeTitle",
    message: "Maximize for deep review",
  }),
  restoreTitle: msg({
    id: "panel.chrome.restoreTitle",
    message: "Restore previous panel size",
  }),
  maximizePanel: msg({
    id: "panel.chrome.maximizePanel",
    message: "Maximize panel",
  }),
  restorePanel: msg({
    id: "panel.chrome.restorePanel",
    message: "Restore panel",
  }),
  closeTitle: msg({
    id: "panel.chrome.closeTitle",
    message: "Close (Esc)—closes the diff window too",
  }),
  closePanel: msg({ id: "panel.chrome.closePanel", message: "Close panel" }),
  updateChecking: msg({
    id: "panel.chrome.updateChecking",
    message: "Checking updates…",
  }),
  updateCurrent: msg({ id: "panel.chrome.updateCurrent", message: "✓ Up to date" }),
  updateFailed: msg({
    id: "panel.chrome.updateFailed",
    message: "⚠ Update check failed",
  }),
  fetchFetching: msg({ id: "panel.fetch.fetching", message: "Fetching origin" }),
  fetchCurrent: msg({
    id: "panel.fetch.current",
    message: "Fetched—origin refs are current",
  }),
  fetchBusy: msg({ id: "panel.fetch.busy", message: "Another fetch is running" }),
  fetchFailed: msg({ id: "panel.fetch.failed", message: "Fetch failed" }),
  fetchNoOrigin: msg({
    id: "panel.fetch.noOrigin",
    message: "No origin remote is configured",
  }),
  fetchAuthentication: msg({
    id: "panel.fetch.authentication",
    message: "Origin authentication failed",
  }),
  fetchNetwork: msg({
    id: "panel.fetch.network",
    message: "Origin is unreachable",
  }),
  fetchRefLock: msg({
    id: "panel.fetch.refLock",
    message: "Git refs are busy—try again in a moment",
  }),
  fetchTimeout: msg({
    id: "panel.fetch.timeout",
    message: "Origin fetch timed out after 30 seconds",
  }),
  fetchUnavailable: msg({
    id: "panel.fetch.unavailable",
    message: "The repository or system Git is unavailable",
  }),
  fetchNow: msg({ id: "panel.fetch.now", message: "Fetch origin now" }),
  fetchFetchingTitle: msg({
    id: "panel.fetch.fetchingTitle",
    message: "Fetching origin…",
  }),
  fetchCurrentTitle: msg({
    id: "panel.fetch.currentTitle",
    message: "Fetched ✓—origin refs are current",
  }),
  fetchBusyTitle: msg({
    id: "panel.fetch.busyTitle",
    message: "Another fetch is running—try again in a moment",
  }),
  fetchFailedTitle: msg({
    id: "panel.fetch.failedTitle",
    message:
      "Fetch failed for an unclassified reason—counts reflect the last successful fetch",
  }),
  fetchNowTitle: msg({
    id: "panel.fetch.nowTitle",
    message: "Fetch origin now—refresh from the remote (read-only; never merges or pushes)",
  }),
  justNow: msg({ id: "time.justNow", message: "just now" }),
  minutesAgo: msg({ id: "time.minutesAgo", message: "{count}m ago" }),
  hoursAgo: msg({ id: "time.hoursAgo", message: "{count}h ago" }),
  daysAgo: msg({ id: "time.daysAgo", message: "{count}d ago" }),
  lastFetch: msg({ id: "panel.upstream.lastFetch", message: "Last fetch: {age}" }),
  noFetch: msg({ id: "panel.upstream.noFetch", message: "No fetch recorded yet" }),
  upstreamSyncedTitle: msg({
    id: "panel.upstream.syncedTitle",
    message: "{local} is in sync with {upstream}.\n{fetch}. Reflects your last fetch.",
  }),
  upstreamDivergedTitle: msg({
    id: "panel.upstream.divergedTitle",
    message:
      "{local} vs {upstream}: {ahead} ahead, {behind} behind.\n{fetch}. Reflects your last fetch.",
  }),
  upstreamSyncedAria: msg({
    id: "panel.upstream.syncedAria",
    message: "In sync with {upstream}",
  }),
  upstreamDivergedAria: msg({
    id: "panel.upstream.divergedAria",
    message: "{ahead} ahead, {behind} behind {upstream}",
  }),
} as const;
