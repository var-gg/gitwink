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

export interface TimelineRepoFill {
  commits: CommitSummary[];
  /** True when these commits were just observed by the file watcher. */
  fresh: boolean;
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
  /** Branch hint when the commit is NOT on the user's currently checked-out branch. */
  branchLabel: string | null;
  isMerge: boolean;
  isTagged: boolean;
  /** Parent commit SHAs in order. Used by the DAG lane drawer. */
  parents: string[];
  /** Full commit message (summary + body). */
  message: string;
  /** Remote-tracking ref shorthand (e.g. "origin/main") whose tip points
   * at this exact commit. Local file read — gitwink never calls fetch.
   * null when no remote ref points here. Separate from branchLabel
   * because remote tip identity is "this commit IS the tip of origin/X",
   * not "this commit is somewhere on origin/X". */
  remoteTipLabel: string | null;
  /** If multiple remote refs point at the same commit (e.g. origin/main
   * and origin/release), this is the count beyond remoteTipLabel. UI
   * renders "+N" after the badge. */
  remoteTipExtraCount: number;
}

export interface BranchInfo {
  name: string;
  tipHash: string;
  isHead: boolean;
  commitCount: number;
  lastActivity: number;
}

/** Snapshot of the current branch's relation to its upstream remote-tracking
 * ref. Computed from local files only — gitwink never calls git fetch, so
 * these counts reflect the user's last fetch, not the live remote. */
export interface UpstreamStatus {
  localBranch: string;
  /** e.g. "origin/main" */
  upstream: string;
  /** Commits on local but not upstream, capped at 99. */
  ahead: number;
  /** Commits on upstream but not local, capped at 99. */
  behind: number;
  aheadCapped: boolean;
  behindCapped: boolean;
  /** FETCH_HEAD mtime in unix seconds, or null if never fetched. */
  lastFetchUnix: number | null;
}

export type WindowDays = 1 | 3 | 7 | 30 | "all";

export interface AuthorTally {
  name: string;
  count: number;
  lastActivity: number;
}

export type ChangedFileStatus =
  | "modified"
  | "new"
  | "renamed"
  | "deleted"
  | "copied"
  | "typechange";

export interface ChangedFile {
  path: string;
  oldPath: string | null;
  insertions: number;
  deletions: number;
  status: ChangedFileStatus;
  isBinary: boolean;
  oldSize: number | null;
  newSize: number | null;
}

export interface CommitFileBlobs {
  oldBase64: string | null;
  newBase64: string | null;
  extension: string;
  isLfs: boolean;
}
