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
   * UI), "removed" = user excluded it (filtered out, won't auto-
   * rediscover). Older cache rows default to "active". */
  status: "active" | "missing" | "removed";
}

/** A recoverable repository exclusion shown in Settings. Excluding only
 * suspends discovery/watching and cached timeline rows; the folder and
 * durable per-repo preferences stay untouched. */
export interface ExcludedRepo {
  path: string;
  name: string;
  excludedAt: number;
  /** null = deliberately not probed while listing (dead shares stay instant). */
  folderAvailable: boolean | null;
}

export interface ReplacementConfirmation {
  canonicalPath: string;
  repoName: string;
  storageIdentity: string;
  refsFingerprint: string;
  lifecycleEpoch: number;
}

export interface RepoStatusPayload {
  canonicalPath: string;
  status: "active" | "missing" | "removed";
}

export interface RepoCatalogChangedPayload {
  canonicalPath: string;
  change: "excluded" | "restored" | "replaced";
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
   * at this exact commit. Read from local refs (refreshed by your IDE/CLI
   * or gitwink's optional auto-fetch). null when no remote ref points here.
   * Separate from branchLabel
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
  /** Reachable-commit count, or null when not yet computed. The branch list
   * loads with this null (counting is a per-branch history walk); the
   * BranchChip fills it in lazily for visible rows via countBranchCommits. */
  commitCount: number | null;
  lastActivity: number;
}

/** Lazily-computed reachable-commit count for one branch ref. */
export interface BranchCommitCount {
  refName: string;
  count: number;
}

/** Snapshot of the current branch's relation to its upstream remote-tracking
 * ref. Computed from local files only — the counts reflect the last fetch
 * (your IDE/CLI, or gitwink's optional auto-fetch on panel open), not a live
 * remote query. */
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

/** Contextual author tallies + per-repo commit counts — mirrors
 * `cache::FilterFacets`. Each facet excludes its own active dimension. */
export interface FilterFacets {
  authors: AuthorTally[];
  repos: RepoCommitCount[];
}

export type FileHistoryTruncation =
  | "match_limit"
  | "scan_limit"
  | "incomplete"
  | "shallow_boundary";

/** File-history rows plus an explicit coverage contract. A null
 * `truncated` is the only state that means every reachable root was
 * exhausted. Mirrors `git::FileHistoryResult`. */
export interface FileHistoryResult {
  rows: CommitSummary[];
  commitsScanned: number;
  truncated: FileHistoryTruncation | null;
  remoteRefsIncluded: number;
  followsRenames: boolean;
}

/** Single-repository walk rows plus an explicit cap contract. `truncated`
 * means the requested branch/time scope contains more rows than `limit`;
 * callers must never present that result as exhaustive history. */
export interface RepoCommitsResult {
  rows: CommitSummary[];
  truncated: boolean;
  /** Ref/revwalk/cutoff uncertainty means this is a truthful partial view. */
  incomplete: boolean;
  limit: number;
  shallow: boolean;
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

/** Installed artifact's update-ownership class. WinGet and direct downloads
 * intentionally share `standalone` because they install the same artifact. */
export type InstallChannel = "standalone" | "scoop" | "store";

/** Stable identity from Rust; UI catalogs own localized provider wording. */
export type UpdateProvider =
  | "githubReleases"
  | "microsoftStore"
  | "scoop"
  | "gitwink";

/** Capability contract for the final install step. Presentation code branches
 * on this union rather than hard-coding Store/Scoop channel names. */
export type UpdateInstallAction =
  | { kind: "inApp"; provider: UpdateProvider }
  | { kind: "externalCommand"; provider: UpdateProvider; command: string };

export interface UpdatePolicy {
  /** Diagnostic identity only; behaviour comes from the capabilities below. */
  channel: InstallChannel;
  checkProvider: UpdateProvider;
  supportsBackgroundChecks: boolean;
  installAction: UpdateInstallAction;
}

/** A pending self-update — mirrors update::AvailableUpdate (Rust). */
export interface AvailableUpdate {
  version: string;
  /** Release notes or a channel-specific update explanation. */
  notes: string;
  mandatory: boolean;
}

/** Updater snapshot for the modal — mirrors commands::UpdateStatePayload. */
export interface UpdateStatePayload {
  /** The pending update, or null when up to date / not yet checked. */
  available: AvailableUpdate | null;
  policy: UpdatePolicy;
}

export interface UpdateProgress {
  phase: "preparing" | "waiting" | "downloading" | "installing" | "restarting";
  /** Normalized completion in the inclusive 0…1 range. */
  progress: number;
}

export interface UpdateCheckNotice {
  kind: "checking" | "current" | "error";
  detail?: string;
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
  /** Free-text commit search. Whitespace-separated terms AND together;
   * each prefixes a Unicode word in commit text/identity/repo name; a 4+
   * char hex term also prefixes the hash. null/empty = no restriction. */
  query?: string | null;
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

/** Demand-driven complete-history coverage for the current repository scope. */
export interface HistoryIndexStatus {
  totalRepos: number;
  completeRepos: number;
  /** Repos that already have an atomically-published full snapshot. */
  publishedRepos: number;
  indexingRepos: number;
  failedRepos: number;
  unavailableRepos: number;
  /** Queued or actively walking repositories. */
  queuedRepos: number;
  commitsScanned: number;
  generation: number;
  /** No queued/active work remains, including terminal partial outcomes. */
  settled: boolean;
  allComplete: boolean;
  lastError: string | null;
}
