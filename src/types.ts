// Shared types mirrored between Rust (commands.rs) and the frontend.
// Keep this file in lock-step with the serde structs on the Rust side.

export interface Repo {
  path: string;
  name: string;
}

export interface ScanProgress {
  root: string;
  found: number;
}

export interface ScanComplete {
  count: number;
}

export interface CommitSummary {
  repoPath: string;
  repoName: string;
  hash: string;
  shortHash: string;
  summary: string;
  author: string;
  email: string;
  timestamp: number;
}

export interface ChangedFile {
  path: string;
  oldPath?: string;
  insertions: number;
  deletions: number;
  status: "modified" | "new" | "renamed" | "deleted";
}
