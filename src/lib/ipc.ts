import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  BranchInfo,
  ChangedFile,
  CommitAround,
  CommitFileBlobs,
  CommitSummary,
  CommitWindow,
  Cursor,
  DiscoveredRepo,
  FilterFacets,
  OrchestratorScanProgress,
  Repo,
  ScanComplete,
  ScanProgress,
  TimelineFilters,
  TimelineInvalidated,
  UpdateStatePayload,
  UpstreamStatus,
  WindowDirection,
} from "../types";

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

export async function listRepos(): Promise<Repo[]> {
  return invoke<Repo[]>("list_repos");
}

export async function discoverRepos(): Promise<number> {
  return invoke<number>("discover_repos");
}

export async function listRecentCommitsCached(
  windowDays: number | null,
): Promise<CommitSummary[]> {
  return invoke<CommitSummary[]>("list_recent_commits_cached", {
    windowDays,
  });
}

export async function recentCommits(
  windowDays: number | null,
): Promise<CommitSummary[]> {
  return invoke<CommitSummary[]>("recent_commits", { windowDays });
}

/** Side-effect-only refill: same git→cache scan as `recentCommits`, but the
 *  backend returns just the merged row count. The windowed timeline re-pulls
 *  rows from the cache, so the full array over IPC was pure overhead. */
export async function refreshRecentCommits(
  windowDays: number | null,
): Promise<number> {
  return invoke<number>("refresh_recent_commits", { windowDays });
}

export async function listBranches(repoPath: string): Promise<BranchInfo[]> {
  return invoke<BranchInfo[]>("list_branches", { repoPath });
}

export async function currentUpstreamStatus(
  repoPath: string,
  branchName: string | null,
): Promise<UpstreamStatus | null> {
  return invoke<UpstreamStatus | null>("current_upstream_status", {
    repoPath,
    branchName,
  });
}

/** Add a repo by absolute path (drag-drop / paste). Backend validates
 * via git2::Repository::discover (so a sub-folder of a repo also works).
 * Throws on non-Git paths so the UI can show an inline error. On
 * success the orchestrator has already emitted `timeline://repo-
 * discovered`, so the listener in App.tsx picks the new row up
 * automatically — the resolved Repo is also returned for callers that
 * want to show synchronous feedback. */
export async function explicitAddRepo(path: string): Promise<DiscoveredRepo> {
  return invoke<DiscoveredRepo>("explicit_add_repo", { path });
}

/** Hide a repo from the panel and prevent auto-rediscovery. Used by
 * the "hide" affordance on missing rows in the RepoChip. The user can
 * always bring it back by dropping/pasting the path again. */
export async function hideRepo(canonicalPath: string): Promise<void> {
  await invoke("hide_repo", { canonicalPath });
}

export async function repoCommits(
  repoPath: string,
  branches: string[] | null,
  windowDays: number | null,
): Promise<CommitSummary[]> {
  return invoke<CommitSummary[]>("repo_commits", {
    repoPath,
    branches,
    windowDays,
  });
}

export async function changedFiles(
  repoPath: string,
  hash: string,
): Promise<ChangedFile[]> {
  return invoke<ChangedFile[]>("changed_files", { repoPath, hash });
}

/** Fire-and-forget prefetch on hover. Errors are swallowed silently. */
export async function prefetchCommit(
  repoPath: string,
  hash: string,
): Promise<void> {
  try {
    await invoke("changed_files", { repoPath, hash });
  } catch {
    /* swallow */
  }
}

/** Phase 6 detail-tier prefetch — warm the changed-files cache for a set of
 * commits (the rows in/near the timeline viewport) so expanding one is
 * instant. Fire-and-forget; the backend skips already-cached commits. */
export async function changedFilesBatch(
  commits: { repoPath: string; hash: string }[],
): Promise<void> {
  try {
    await invoke("changed_files_batch", { commits });
  } catch {
    /* swallow */
  }
}

/** Context-line count that yields a whole-file diff: far larger than any
 * real file, so git emits the entire file as context with add/delete
 * tinting intact. Not cached (only the default ±3 view is). */
export const WHOLE_FILE_CONTEXT = 1_000_000;

export async function fileDiff(
  repoPath: string,
  hash: string,
  filePath: string,
  /** Unified-diff context lines. 3 = default hunk view; larger expands
   * context; a very large value (see WHOLE_FILE_CONTEXT) yields a
   * whole-file diff. Only the default is cached by the backend. */
  contextLines = 3,
): Promise<string> {
  return invoke<string>("file_diff", { repoPath, hash, filePath, contextLines });
}

export async function commitFileBlobs(
  repoPath: string,
  hash: string,
  filePath: string,
  oldPath: string | null,
): Promise<CommitFileBlobs> {
  return invoke<CommitFileBlobs>("commit_file_blobs", {
    repoPath,
    hash,
    filePath,
    oldPath,
  });
}

export async function openDiff(
  repoPath: string,
  repoName: string,
  hash: string,
  shortHash: string,
  summary: string,
  filePath: string,
): Promise<void> {
  await invoke("open_diff", {
    repoPath,
    repoName,
    hash,
    shortHash,
    summary,
    filePath,
  });
}

export interface DiffOpenPayload {
  repoPath: string;
  repoName: string;
  hash: string;
  shortHash: string;
  summary: string;
  filePath: string;
}

export async function takePendingDiffOpen(): Promise<DiffOpenPayload | null> {
  return invoke<DiffOpenPayload | null>("take_pending_diff_open");
}

export async function dismissPanel(): Promise<void> {
  await invoke("dismiss_panel");
}

/** Tell the backend whether the panel should resist blur-dismiss. Set
 * true while the empty-state add-repo screen is showing or a native
 * folder picker is open — focus legitimately leaves the panel in both
 * cases without the user meaning to dismiss it. */
export async function setPanelSticky(sticky: boolean): Promise<void> {
  await invoke("set_panel_sticky", { sticky });
}

/** Pull the orchestrator's current scan state. Used at startup: the
 * `scan-progress` 'complete' event can fire before this window's
 * listener registers (fast run on a repo-light machine), which would
 * otherwise leave the "Scanning…" indicator stuck on. */
export async function getScanState(): Promise<boolean> {
  return invoke<boolean>("get_scan_state");
}

export async function getPinnedRepos(): Promise<string[]> {
  return invoke<string[]>("get_pinned_repos");
}

export async function setPinnedRepos(repos: string[]): Promise<void> {
  await invoke("set_pinned_repos", { repos });
}

/** Saved branch selection for one repo — restored when the user
 * re-enters single-repo mode. An empty array means "all branches".
 * Persisted per repo in settings.json. */
export async function getBranchSelection(repoPath: string): Promise<string[]> {
  return invoke<string[]>("get_branch_selection", { repoPath });
}

export async function setBranchSelection(
  repoPath: string,
  selection: string[],
): Promise<void> {
  await invoke("set_branch_selection", { repoPath, selection });
}

export async function onScanProgress(
  cb: (p: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>("discovery://progress", (e) => cb(e.payload));
}

export async function onScanComplete(
  cb: (p: ScanComplete) => void,
): Promise<UnlistenFn> {
  return listen<ScanComplete>("discovery://complete", (e) => cb(e.payload));
}

/** v0.1.1 orchestrator scan progress. Fires roughly every 500ms while
 * the prewarm task is running and once more with state="complete" at
 * the end. Use this for the panel progress strip and tray tooltip
 * mirroring; the older `onScanProgress`/`onScanComplete` channels are
 * still supported for the manual `discover_repos` IPC path. */
export async function onOrchestratorProgress(
  cb: (p: OrchestratorScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<OrchestratorScanProgress>("scan-progress", (e) => cb(e.payload));
}

/** v0.1.1 per-repo discovery event. Fires once per validated repo as
 * the orchestrator caches it. Frontend appends to its repo list and
 * triggers a recent-commits refetch if the repo isn't already known. */
export async function onRepoDiscovered(
  cb: (p: DiscoveredRepo) => void,
): Promise<UnlistenFn> {
  return listen<DiscoveredRepo>("timeline://repo-discovered", (e) => cb(e.payload));
}

// ----- windowed-pull timeline (Phase 1-3) -----

/** One keyset-paginated page of the all-repos timeline. `cursor` null reads
 * from the top (newest); `direction` walks "older" (down) or "newer" (up). */
export async function listCommitsWindow(
  filters: TimelineFilters,
  cursor: Cursor | null,
  direction: WindowDirection,
  limit: number,
): Promise<CommitWindow> {
  return invoke<CommitWindow>("list_commits_window", {
    filters,
    cursor,
    direction,
    limit,
  });
}

/** A window of commits centred on an anchor cursor — filter-change viewport
 * recovery. The anchor need not survive the new filter; `anchorFound` says
 * whether it did, and the rows centre on where it sits (or would sit). */
export async function listCommitsAroundAnchor(
  filters: TimelineFilters,
  anchor: Cursor,
  before: number,
  after: number,
): Promise<CommitAround> {
  return invoke<CommitAround>("list_commits_around_anchor", {
    filters,
    anchor,
    before,
    after,
  });
}

/** A window of commits centred on a 0-based rank — the random-access
 * scrollbar's jump-load. `baseIndex` in the result places it in the
 * `count`-tall virtual scroll space. */
export async function listCommitsAtRank(
  filters: TimelineFilters,
  rank: number,
  before: number,
  after: number,
): Promise<CommitAround> {
  return invoke<CommitAround>("list_commits_at_rank", {
    filters,
    rank,
    before,
    after,
  });
}

/** Total commits under `filters` — the timeline's count label. */
export async function countCommits(filters: TimelineFilters): Promise<number> {
  return invoke<number>("count_commits", { filters });
}

/** The current commit generation. The windowed timeline pins this as its
 * `viewGeneration` so the scanner's later inserts don't disturb the page
 * sequence it is showing. */
export async function getTimelineGeneration(): Promise<number> {
  return invoke<number>("get_timeline_generation");
}

/** The timeline's filter facets (author tallies + per-repo commit counts)
 * under `filters` — the AuthorsChip + RepoChip count sources. The windowed
 * timeline holds no full client-side commit array to tally itself. */
export async function listFilterFacets(
  filters: TimelineFilters,
): Promise<FilterFacets> {
  return invoke<FilterFacets>("list_filter_facets", { filters });
}

/** Lightweight scanner→UI signal: a new generation landed (a `git commit`
 * in a watched repo, a discovery sweep, …). The windowed timeline re-pulls
 * the affected windows from the cache rather than receiving commit arrays. */
export async function onTimelineInvalidated(
  cb: (p: TimelineInvalidated) => void,
): Promise<UnlistenFn> {
  return listen<TimelineInvalidated>("timeline://invalidated", (e) =>
    cb(e.payload),
  );
}

/** Fires when the panel is summoned (tray click / global hotkey). The
 * webview is only un-hidden, never re-created, so the bootstrap commit
 * fetch runs once per launch — the frontend uses this to re-pull commits
 * as a fallback for anything the live file-watcher missed. */
export async function onPanelShown(cb: () => void): Promise<UnlistenFn> {
  return listen("panel://shown", () => cb());
}

// ----- self-update -----

/** Snapshot the updater state for the modal: the pending update (if any)
 * plus whether this is a Scoop install. */
export async function updateGetState(): Promise<UpdateStatePayload> {
  return invoke<UpdateStatePayload>("update_get_state");
}

/** Download + install the pending update, then relaunch. Resolves only
 * on failure — on success the app process is replaced before the
 * promise settles. */
export async function updateInstall(): Promise<void> {
  await invoke("update_install");
}

/** "Skip vX" — suppress the update indicator for the current version. */
export async function updateSkip(): Promise<void> {
  await invoke("update_skip");
}

/** "Later" — hide the update indicator for 24h. */
export async function updateSnooze(): Promise<void> {
  await invoke("update_snooze");
}

/** Backend asks the panel to open the update modal — fired by the tray
 * "Update available" item, a manual check that found a release, or a
 * manual check on a Scoop install. */
export async function onUpdateShowModal(cb: () => void): Promise<UnlistenFn> {
  return listen("update://show-modal", () => cb());
}

/** A manual check found nothing — the panel shows a brief "up to date". */
export async function onUpdateNone(cb: () => void): Promise<UnlistenFn> {
  return listen("update://none", () => cb());
}
