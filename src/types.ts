// Shared types mirrored between Rust (commands.rs) and the frontend.
// Keep this file in lock-step with the serde structs on the Rust side.

export interface Repo {
  /** `repos.rowid` — the integer the windowed timeline filters by
   * (`TimelineFilters.repoIds`). 0 for a repo not yet resolved via
   * `listRepos` (e.g. one just optimistically added from a discovery
   * event); a `listRepos` refresh backfills the real id. */
  id: number;
  path: string;
  name: string;
  /** Lifecycle status. "active" = on disk and validates as a git repo,
   * "missing" = previously seen but path no longer exists (greyed in
   * UI), "removed" = user explicitly hid it (filtered out, won't auto-
   * rediscover). Older cache rows default to "active". */
  status: "active" | "missing" | "removed";
}

export interface ScanProgress {
  root: string;
  found: number;
}

export interface ScanComplete {
  count: number;
}

/** New v0.1.1 orchestrator → frontend channel. Replaces the old
 * `scan://progress` / `scan://complete` pair for the tiered scanner.
 * `state` is "scanning" while the prewarm task is alive and "complete"
 * once it finishes; `repos_found` is the validated-repo count so far. */
export interface OrchestratorScanProgress {
  reposFound: number;
  state: "scanning" | "complete";
  errors: number;
}

/** New v0.1.1 orchestrator → frontend channel. Fired once per validated
 * repo as the prewarm task discovers and caches it. `source` is a debug
 * hint (vscode / cursor / git_config_safe / fs_walk / manual / ...) —
 * UI surfaces it in tooltips only. */
export interface DiscoveredRepo {
  path: string;
  name: string;
  source: string;
  confidence: number;
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
  /** Display name. Local: shorthand ("main"). Remote: includes remote prefix ("origin/main"). */
  name: string;
  /** Fully-qualified ref name. Use this as the wire identifier when
   * filtering — disambiguates a local "main" from a remote "origin/main". */
  refName: string;
  /** "local" | "remote" — frontend groups by this in the BranchChip. */
  kind: "local" | "remote";
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

/** Per-repo commit count under the active filters — mirrors
 * `cache::RepoCommitCount`. */
export interface RepoCommitCount {
  repoId: number;
  count: number;
}

/** The timeline's filter facets (author tallies + per-repo commit counts)
 * — mirrors `cache::FilterFacets`. */
export interface FilterFacets {
  authors: AuthorTally[];
  repos: RepoCommitCount[];
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

/** A pending self-update — mirrors update::AvailableUpdate (Rust). */
export interface AvailableUpdate {
  version: string;
  /** Release notes / changelog text from the GitHub release. */
  notes: string;
}

/** Updater snapshot for the modal — mirrors commands::UpdateStatePayload. */
export interface UpdateStatePayload {
  /** The pending update, or null when up to date / not yet checked. */
  available: AvailableUpdate | null;
  /** True for Scoop installs — the modal shows a `scoop update` hint
   * instead of an in-app "Update now" button. */
  scoop: boolean;
}

// ----- windowed-pull timeline (mirrors the Rust cache.rs structs) -----

/** A keyset cursor into the timeline's total order. Opaque to the UI —
 * obtained from a `CommitWindow` and handed back to fetch adjacent pages.
 * Mirrors `cache::Cursor`. */
export interface Cursor {
  sortTs: number;
  repoPath: string;
  hash: string;
}

/** Which way a window query reads from its cursor. */
export type WindowDirection = "older" | "newer";

/** Server-side timeline filters — mirrors `cache::TimelineFilters`. An
 * absent / null field means "no restriction". */
export interface TimelineFilters {
  /** Restrict to these repo ids (`Repo.id`). null/absent = all repos. */
  repoIds?: number[] | null;
  /** Restrict to these author names. null/absent = all authors. */
  authors?: string[] | null;
  /** Only commits at/after this unix-seconds timestamp. null = all time. */
  since?: number | null;
  /** MVCC-lite snapshot pin: only commits first seen at/before this
   * generation are visible, so the scanner's later inserts don't disturb
   * the page sequence. null = no pin. */
  viewGeneration?: number | null;
}

/** One keyset-paginated page of the timeline — mirrors `cache::CommitWindow`.
 * `rows` is always newest-first. */
export interface CommitWindow {
  rows: CommitSummary[];
  startCursor: Cursor | null;
  endCursor: Cursor | null;
  hasNewer: boolean;
  hasOlder: boolean;
}

/** A window of commits centred on an anchor, plus where it sits in the
 * filtered total order — mirrors `cache::CommitAround`. `rows` is
 * newest-first; `baseIndex` is the global rank (0-based) of `rows[0]`, so
 * the UI can drop the window into a `count`-tall virtual scroll space. */
export interface CommitAround {
  rows: CommitSummary[];
  /** whether the anchor commit itself survived the filter */
  anchorFound: boolean;
  baseIndex: number;
  startCursor: Cursor | null;
  endCursor: Cursor | null;
  hasNewer: boolean;
  hasOlder: boolean;
}

/** Lightweight scanner→UI invalidation signal — mirrors
 * `cache::TimelineInvalidated`. The windowed timeline re-pulls affected
 * windows from the cache instead of receiving commit arrays. */
export interface TimelineInvalidated {
  generation: number;
  inserted: number;
  /** Ghost commits reconciled away (history was rewritten under us). */
  deleted: number;
  repoPath: string;
}
