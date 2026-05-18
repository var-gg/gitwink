import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  CommitSummary,
  Repo,
  ScanComplete,
  ScanProgress,
  TimelineRepoFill,
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
