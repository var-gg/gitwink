import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  BranchInfo,
  ChangedFile,
  CommitFileBlobs,
  CommitSummary,
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

export async function onTimelineRepoFill(
  cb: (p: TimelineRepoFill) => void,
): Promise<UnlistenFn> {
  return listen<TimelineRepoFill>("timeline://repo-fill", (e) => cb(e.payload));
}
