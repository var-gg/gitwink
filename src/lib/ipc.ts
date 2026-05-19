import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  BranchInfo,
  ChangedFile,
  CommitFileBlobs,
  CommitSummary,
  DiscoveredRepo,
  OrchestratorScanProgress,
  Repo,
  ScanComplete,
  ScanProgress,
  TimelineRepoFill,
  UpstreamStatus,
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

export async function fileDiff(
  repoPath: string,
  hash: string,
  filePath: string,
): Promise<string> {
  return invoke<string>("file_diff", { repoPath, hash, filePath });
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

export async function getPinnedRepos(): Promise<string[]> {
  return invoke<string[]>("get_pinned_repos");
}

export async function setPinnedRepos(repos: string[]): Promise<void> {
  await invoke("set_pinned_repos", { repos });
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

export async function onTimelineRepoFill(
  cb: (p: TimelineRepoFill) => void,
): Promise<UnlistenFn> {
  return listen<TimelineRepoFill>("timeline://repo-fill", (e) => cb(e.payload));
}
