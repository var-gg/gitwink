import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Repo, ScanComplete, ScanProgress } from "../types";

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

export async function listRepos(): Promise<Repo[]> {
  return invoke<Repo[]>("list_repos");
}

export async function discoverRepos(): Promise<number> {
  return invoke<number>("discover_repos");
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
