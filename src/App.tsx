import {
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { AuthorsChip } from "./components/AuthorsChip";
import { BranchChip } from "./components/BranchChip";
import { ContextMenu, type MenuItem } from "./components/ContextMenu";
import { RepoChip } from "./components/RepoChip";
import { SearchBar } from "./components/SearchBar";
import { Timeline } from "./components/Timeline";
import {
  TimelineWindowed,
  type ExpansionControl,
  type SearchControl,
} from "./components/TimelineWindowed";
import {
  TimeRangeChip,
  WINDOW_DAY_PRESETS,
} from "./components/TimeRangeChip";
import {
  ackAutoFetchNotice,
  currentUpstreamStatus,
  dismissPanel,
  excludeRepo,
  explicitAddRepo,
  fetchRepoNow,
  fileHistory as fetchFileHistory,
  getBranchSelection,
  repoRefsFingerprint,
  getPinnedRepos,
  getScanState,
  listBranches,
  listFilterFacets,
  listRepos,
  onFileHistoryOpen,
  onOrchestratorProgress,
  onPanelShown,
  onRepoCatalogChanged,
  onRepoDiscovered,
  onRepoStatus,
  onTimelineInvalidated,
  onUpdateIndicator,
  onUpdateNone,
  onUpdateShowModal,
  repoCommits,
  setBranchSelection as saveBranchSelection,
  setPanelSticky,
  setPinnedRepos as savePinnedRepos,
  updateGetState,
  updateRefreshIndicator,
} from "./lib/ipc";
import {
  broadcastSettings,
  getCurrentSettings,
  usePanelPinned,
} from "./lib/settings";
import { repoCatalogReducer } from "./lib/repoCatalog";
import {
  pruneRepoFromViewHistory,
  type ViewSnapshot,
} from "./lib/viewHistory";
import { UpdateModal } from "./components/UpdateModal";
import type {
  AuthorTally,
  BranchInfo,
  CommitSummary,
  UpdateStatePayload,
  UpstreamStatus,
  WindowDays,
} from "./types";
import "./styles.css";

function startDrag(e: React.PointerEvent<HTMLElement>) {
  if (e.button !== 0) return;
  const target = e.target as HTMLElement | null;
  // Don't start a drag if the press landed on a clickable control or
  // inside an open chip dropdown (incl. its scrollbars / inputs / pin
  // buttons). The user is interacting with the dropdown, not the window.
  if (target?.closest("button, input, .chip-dropdown, [data-no-drag]")) return;
  void getCurrentWindow().startDragging();
}

function formatFetchAge(unixSeconds: number): string {
  const ageSec = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (ageSec < 60) return "just now";
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86_400) return `${Math.floor(ageSec / 3600)}h ago`;
  return `${Math.floor(ageSec / 86_400)}d ago`;
}

interface UpstreamBadgeProps {
  status: UpstreamStatus;
}

/** Tiny inline status badge: shows `synced` / `↑N` / `↓N` / `↑N ↓N` next
 * to the BranchChip in single-repo mode. Reads from local refs only — the
 * counts reflect your last fetch (auto-fetch can refresh them on panel open;
 * gitwink still never merges, pushes, or rewrites). The tooltip spells out
 * the last-fetch caveat so users don't expect live remote state. */
function UpstreamBadge({ status }: UpstreamBadgeProps) {
  const synced = status.ahead === 0 && status.behind === 0;
  const aheadStr = status.ahead.toString() + (status.aheadCapped ? "+" : "");
  const behindStr = status.behind.toString() + (status.behindCapped ? "+" : "");
  const fetchHint = status.lastFetchUnix
    ? `Last fetch: ${formatFetchAge(status.lastFetchUnix)}`
    : "No fetch recorded yet";
  const title = synced
    ? `${status.localBranch} is in sync with ${status.upstream}.\n${fetchHint}. Reflects your last fetch — auto-fetch can refresh it on panel open.`
    : `${status.localBranch} vs ${status.upstream}: ${status.ahead} ahead, ${status.behind} behind.\n${fetchHint}. Reflects your last fetch — auto-fetch can refresh it on panel open.`;

  return (
    <span
      className={
        "upstream-badge" + (synced ? " upstream-badge-synced" : " upstream-badge-diverged")
      }
      title={title}
      aria-label={
        synced
          ? `In sync with ${status.upstream}`
          : `${status.ahead} ahead, ${status.behind} behind ${status.upstream}`
      }
    >
      {synced ? (
        // Compact: glyph only. Full ref name lives in title/aria-label so
        // the header doesn't overflow when both BranchChip and this badge
        // share space.
        <span className="upstream-badge-check" aria-hidden="true">
          ✓
        </span>
      ) : (
        <>
          {status.ahead > 0 && <span className="upstream-badge-ahead">↑{aheadStr}</span>}
          {status.behind > 0 && (
            <span className="upstream-badge-behind">↓{behindStr}</span>
          )}
        </>
      )}
    </span>
  );
}

function toWindowParam(w: WindowDays): number | null {
  return w === "all" ? null : (w as number);
}

/** Smallest TimeRangeChip preset that still covers `commitTs`, keeping the
 * current pick when it already does. The warp uses this so the landing
 * view's time window can't hide the commit it just jumped to. The 6h
 * margin keeps a commit sitting right at a cutoff from falling out when
 * the backend recomputes `since` at fetch time. */
function windowCovering(current: WindowDays, commitTs: number): WindowDays {
  if (current === "all") return "all";
  const ageDays = (Date.now() / 1000 - commitTs) / 86_400 + 0.25;
  if (ageDays < current) return current;
  for (const preset of WINDOW_DAY_PRESETS) {
    if (ageDays < preset) return preset;
  }
  return "all";
}

/** Cap on each direction of the view-history stack — deep enough to never bite
 *  in practice, bounded so a long session can't grow it without limit. */
const VIEW_HISTORY_MAX = 50;

/** Tolerant repo-path equality: the backend's cache key and the frontend's
 *  selected path can differ in separator (\ vs /) or case on Windows. */
function samePath(a: string, b: string): boolean {
  if (a === b) return true;
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return norm(a) === norm(b);
}

/** Set-equality for the RepoChip's multi-selection ("all" or a path list). */
function sameRepoSelection(
  a: string[] | "all",
  b: string[] | "all",
): boolean {
  if (a === "all" || b === "all") return a === b;
  if (a.length !== b.length) return false;
  const set = new Set(b);
  return a.every((p) => set.has(p));
}

function App() {
  const [scanning, setScanning] = useState(false);
  const [commits, setCommits] = useState<CommitSummary[] | null>(null);
  // True when the single-repo commits fetch itself FAILED (repo moved, drive
  // disconnected) — distinct from a legitimately empty result, so the view
  // can say "couldn't open" instead of silently keeping stale rows.
  const [commitsError, setCommitsError] = useState(false);
  // Signature of the view the current `commits` belong to. A SIGNATURE change
  // (repo/branch/window/file scope) clears to the loading state so another
  // repo's rows can never render under the new header; a plain refreshNonce
  // re-pull keeps the rows (no flash).
  const commitsSigRef = useRef("");
  // Refs fingerprint the current `commits` were walked from — a same-view
  // re-pull with an unchanged fingerprint skips the multi-MB IPC re-ship.
  const commitsRefsFpRef = useRef<string | null>(null);
  const [allRepos, dispatchRepoCatalog] = useReducer(repoCatalogReducer, []);
  const [pinnedRepos, setPinnedRepos] = useState<string[]>([]);
  // All-repos filter facets — the windowed timeline keeps no full client-
  // side commit array, so the AuthorsChip list + the RepoChip per-repo
  // counts come from a backend facet.
  const [authorsAll, setAuthorsAll] = useState<AuthorTally[]>([]);
  const [repoCounts, setRepoCounts] = useState<Map<number, number>>(
    () => new Map(),
  );

  const [windowDays, setWindowDays] = useState<WindowDays>(7);
  const [selectedRepoPath, setSelectedRepoPath] = useState<string | null>(null);
  const [selectedRepoPaths, setSelectedRepoPaths] = useState<string[] | "all">(
    "all",
  );
  const [selectedAuthors, setSelectedAuthors] = useState<string[] | "all">(
    "all",
  );

  // Single-repo mode state.
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [selectedBranches, setSelectedBranches] = useState<string[] | "all">(
    "all",
  );
  const [upstream, setUpstream] = useState<UpstreamStatus | null>(null);

  // Bumped each time the panel is summoned — the windowed timeline, the
  // author facet, and the single-repo commits effect all depend on it, so
  // re-showing the panel re-pulls (covering anything the watcher missed).
  const [refreshNonce, setRefreshNonce] = useState(0);

  // Bumped only when the VIEWED repo's refs actually change under us (a
  // watcher invalidation for that repo — a local commit, or an auto-fetch
  // landing). The branch LIST and the upstream badge (ahead/behind +
  // fetch-age) depend on this, NOT on every summon, so they refresh exactly
  // when refs move and stay cheap on a plain re-show.
  const [refsNonce, setRefsNonce] = useState(0);

  // Explicit "fetch now" button state — idle → fetching → (ok|failed|busy),
  // decaying back to idle so the glyph doesn't stick. Presentation is pinned
  // to the repo that was actually fetched: switching repos resets to idle
  // (below), and a resolving fetch for a repo you've left is dropped.
  const [fetchNow, setFetchNow] = useState<
    "idle" | "fetching" | "ok" | "failed" | "busy"
  >("idle");
  const fetchNowTimerRef = useRef<number | null>(null);

  // File-history scope: when set, single-repo mode shows the commits that
  // touched this file (a live capped walk), ignoring the branch/author/time
  // lenses. Cleared by the chip's ✕, Esc, or picking a different repo.
  const [fileHistory, setFileHistory] = useState<{
    repoPath: string;
    filePath: string;
  } | null>(null);

  // One-time disclosure: gitwink now fetches the open repo's remote on view.
  // Show it once while auto-fetch is on and unacknowledged — this is also the
  // upgrade-time heads-up for anyone who installed under the old no-network
  // framing. Read from the (already-loaded) settings snapshot at mount.
  const [showAutoFetchNotice, setShowAutoFetchNotice] = useState(() => {
    const s = getCurrentSettings();
    return s.autoFetchOnShow && !s.autoFetchNoticeSeen;
  });
  const dismissAutoFetchNotice = useCallback(() => {
    setShowAutoFetchNotice(false);
    void ackAutoFetchNotice().catch(() => {});
  }, []);

  // ----- commit search (the `/` summon) + warp -----
  // `searchInput` is the live keystrokes; `searchQuery` is its debounced
  // mirror that actually drives the windowed query. While a non-empty
  // query is active the timeline body becomes the result list and the
  // time/author/branch chips are bypassed (they are view lenses — hiding
  // the commit you're hunting behind "30d" is the frustration search
  // exists to kill). The repo scope IS respected.
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchInput, setSearchInput] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [searchCount, setSearchCount] = useState<number | null>(null);
  const [searchFocusNonce, setSearchFocusNonce] = useState(0);
  // One-time "here's what this can do" tip, shown once the first repo's
  // commits are on screen. Persisted in localStorage (frontend-only, no
  // settings round-trip) — the glance loop's power features (search, file
  // history, copy-for-AI) are otherwise discoverable only by accident.
  const [firstRunTipSeen, setFirstRunTipSeen] = useState(
    () =>
      typeof window !== "undefined" &&
      window.localStorage.getItem("gitwink.firstRunTipSeen") === "1",
  );
  const dismissFirstRunTip = useCallback(() => {
    setFirstRunTipSeen(true);
    try {
      window.localStorage.setItem("gitwink.firstRunTipSeen", "1");
    } catch {}
  }, []);
  // Unified view history: a browser-style back/forward stack over the WHOLE
  // view (repo scope, filters, file-history, search). Every discrete
  // navigation (warp, file-history open, repo pick, scope widen) pushes the
  // outgoing view onto `viewBack` and clears `viewFwd`; Alt+←/→, the header
  // ◄►, the file-history chip, and Esc's back rung all step through it. This
  // subsumes the old one-deep warp/file-history returns.
  const [viewBack, setViewBack] = useState<ViewSnapshot[]>([]);
  const [viewFwd, setViewFwd] = useState<ViewSnapshot[]>([]);
  const [warpAnchor, setWarpAnchor] = useState<{
    hash: string;
    nonce: number;
  } | null>(null);
  const warpNonceRef = useRef(0);
  const searchControlRef = useRef<SearchControl | null>(null);
  // The mounted timeline's expansion-collapse control — an explicit rung in
  // the Esc cascade below, so "Esc closes the expansion first" can't lose a
  // window-listener registration-order race.
  const expansionControlRef = useRef<ExpansionControl | null>(null);
  // Live mirror of selectedRepoPath for the once-mounted panel-shown / fetch
  // handlers (which would otherwise close over a stale value).
  const selectedRepoPathRef = useRef<string | null>(null);
  selectedRepoPathRef.current = selectedRepoPath;
  // Live snapshot of the current view, so the history helpers (and the
  // once-mounted file-history listener, a stale closure) can capture the view
  // being replaced without threading every piece of state through deps.
  const viewRef = useRef<ViewSnapshot>({
    repoPath: null,
    repoPaths: "all",
    branches: "all",
    windowDays: 7,
    authors: "all",
    fileHistory: null,
    searchOpen: false,
    searchInput: "",
  });
  viewRef.current = {
    repoPath: selectedRepoPath,
    repoPaths: selectedRepoPaths,
    branches: selectedBranches,
    windowDays,
    authors: selectedAuthors,
    fileHistory,
    searchOpen,
    searchInput,
  };
  // One-shot branch selection for the NEXT repo change, consumed by the
  // branch-selection effect INSTEAD of the repo's disk-saved selection.
  // Two writers: a warp forces "all" (the target commit may not be reachable
  // from the saved filter), and a history restore (applyView) carries the
  // snapshot's own selection — so Back/Forward re-lands on the branches the
  // user was actually looking at, not whatever the repo's latest save says.
  const pendingBranchesRef = useRef<string[] | "all" | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => setSearchQuery(searchInput), 150);
    return () => window.clearTimeout(timer);
  }, [searchInput]);

  const searching = searchOpen && searchQuery.trim().length > 0;

  const [openChip, setOpenChip] = useState<
    "repo" | "time" | "authors" | "branch" | null
  >(null);

  // Right-click on the panel header (empty space / drag handle / icon /
  // status / upstream badge) opens this — currently a single "Settings…"
  // entry, mirroring the tray menu. Chips and the close button keep
  // their own click behaviour.
  const [headerCtxMenu, setHeaderCtxMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);

  // Reactive panel pin state — drives the header pin button glyph + title
  // and re-renders this component whenever the pin flag flips.
  const pinned = usePanelPinned();

  // Drop/paste add-repo flow: inline feedback only, no modal.
  // `addError` clears itself after 4s so a typo'd path doesn't linger.
  const [addError, setAddError] = useState<string | null>(null);

  // True while the native folder picker is open. The picker steals OS
  // focus, which would otherwise blur-dismiss the panel mid add-repo.
  const [dialogOpen, setDialogOpen] = useState(false);

  // Self-update modal — populated when the backend asks the panel to
  // surface it (tray "Update available" item / a manual check). null =
  // closed. The modal never auto-pops; the tray dot is the only passive
  // cue.
  const [updateModal, setUpdateModal] = useState<UpdateStatePayload | null>(
    null,
  );
  // Available-update version (or null) for the passive header-icon badge —
  // kept in sync with the tray dot via the `update://indicator` event.
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  // Transient "you're up to date" line after a manual check found nothing.
  const [upToDate, setUpToDate] = useState(false);

  // Open the update modal from the header badge — same path as the tray
  // "Update available" item.
  const openUpdateModal = useCallback(() => {
    void updateGetState()
      .then((st) => setUpdateModal(st))
      .catch(() => {});
  }, []);

  const singleMode = selectedRepoPath != null;

  // ----- bootstrap -----
  useEffect(() => {
    let mounted = true;
    let unProgress: UnlistenFn | undefined;
    let unDiscovered: UnlistenFn | undefined;
    let unStatus: UnlistenFn | undefined;
    let unCatalogChanged: UnlistenFn | undefined;
    let unShown: UnlistenFn | undefined;
    let unTimelineInvalidated: UnlistenFn | undefined;
    let unFileHistory: UnlistenFn | undefined;
    let unUpdateModal: UnlistenFn | undefined;
    let unUpdateNone: UnlistenFn | undefined;
    let unUpdateIndicator: UnlistenFn | undefined;

    (async () => {
      try {
        const repos = await listRepos();
        if (mounted) {
          dispatchRepoCatalog({ type: "replace", repos });
        }
      } catch {}

      try {
        const pins = await getPinnedRepos();
        if (mounted) setPinnedRepos(pins);
      } catch {}

      // Orchestrator owns discovery now — we just listen.
      // `scanning` is the UI flag for the progress strip + tray; the
      // tray icon's own tooltip is updated by Rust directly.
      //
      // Pull the real scan state first: the `scan-progress` 'complete'
      // event can fire before this listener registers (a fast run on a
      // repo-light machine), which would otherwise leave "Scanning…"
      // stuck on forever. The listener below still catches state changes
      // that happen after this point.
      try {
        const st = await getScanState();
        if (mounted) setScanning(st);
      } catch {
        if (mounted) setScanning(true);
      }
      unProgress = await onOrchestratorProgress((p) => {
        if (!mounted) return;
        setScanning(p.state === "scanning");
        // Scan finished — refresh the repo list so newly-found repos carry
        // their real ids (needed to filter the windowed timeline by repo)
        // and bump the nonce so the timeline + author facet re-pull.
        if (p.state === "complete") {
          void listRepos()
            .then((repos) => {
              if (mounted) {
                dispatchRepoCatalog({ type: "replace", repos });
              }
            })
            .catch(() => {});
          setRefreshNonce((n) => n + 1);
        }
      });

      // Panel summoned — re-pull commits as a fallback for anything the
      // live file-watcher missed (a missed event, a repo whose watcher
      // never attached). The webview persists across hide/show, so this
      // is the only re-fetch trigger besides a filter change.
      unShown = await onPanelShown(() => {
        if (!mounted) return;
        setRefreshNonce((n) => n + 1);
        // Single-repo mode only (repo != null): optionally fetch the viewed
        // repo so a teammate's just-pushed commit surfaces. The backend gates
        // on the auto_fetch_on_show setting + a per-repo cooldown and runs it
        // non-blocking; the resulting ref update flows back through the
        // watcher → timeline-invalidated listener below.
        const repo = selectedRepoPathRef.current;
        if (repo) {
          void invoke("maybe_fetch_repo", { repoPath: repo }).catch(() => {});
        }
      });

      // Single-repo timeline doesn't subscribe to cache invalidations (it
      // re-pulls on refreshNonce), so a background change — a local commit, or
      // the auto-fetch above landing a teammate's commit — wouldn't surface
      // until the next summon. Bump the nonce so it updates live. All-repos
      // mode is left to TimelineWindowed's own invalidation listener.
      unTimelineInvalidated = await onTimelineInvalidated((p) => {
        if (!mounted) return;
        const cur = selectedRepoPathRef.current;
        if (!cur) return;
        // Only react to the repo we're actually viewing — the event carries
        // its own repoPath, so an unrelated repo's change can't trigger a
        // needless rescan of the open one.
        if (p.repoPath && !samePath(p.repoPath, cur)) return;
        setRefreshNonce((n) => n + 1); // commits re-pull
        setRefsNonce((n) => n + 1); // branch list + upstream badge re-eval
      });

      // A diff window asked to show a file's history — enter single-repo mode
      // for that repo and scope the timeline to the file. The commits effect
      // sees `fileHistory` and fetches the file's history instead.
      unFileHistory = await onFileHistoryOpen((p) => {
        if (!mounted) return;
        // Re-clicking 🕘 for the file already on screen is a refresh, not a
        // navigation — no dead history entry. refsNonce is what the
        // file-history effect keys on (refreshNonce deliberately isn't).
        const v = viewRef.current;
        if (
          v.fileHistory &&
          v.fileHistory.repoPath === p.repoPath &&
          v.fileHistory.filePath === p.filePath
        ) {
          setRefsNonce((n) => n + 1);
          return;
        }
        // Record the view we're leaving so back returns to it (works even when
        // opening a second file's history from within file-history mode).
        pushView();
        setSearchOpen(false);
        setSelectedRepoPath(p.repoPath);
        setFileHistory({ repoPath: p.repoPath, filePath: p.filePath });
        setRefreshNonce((n) => n + 1);
      });

      // Updater: backend asks the panel to surface the modal (tray
      // "Update available" item, a manual check hit, or a Scoop install).
      unUpdateModal = await onUpdateShowModal(async () => {
        try {
          const st = await updateGetState();
          if (mounted) setUpdateModal(st);
        } catch {}
      });
      // A manual check found nothing — show a brief "up to date" line.
      unUpdateNone = await onUpdateNone(() => {
        if (!mounted) return;
        setUpToDate(true);
        window.setTimeout(() => setUpToDate(false), 3000);
      });
      // Header badge: mirror the tray's "update available" indicator. Register
      // the listener first, then ask the backend to re-emit the current gated
      // indicator (respects skip / snooze) so a badge found before the panel
      // mounted shows up without waiting for the next check.
      unUpdateIndicator = await onUpdateIndicator((version) => {
        if (mounted) setUpdateVersion(version);
      });
      void updateRefreshIndicator().catch(() => {});

      // Per-repo discovery: merge into allRepos so the repo chip
      // dropdown lights up as repos are validated. Refresh cached
      // commits opportunistically so the timeline picks up rows from
      // the newly-discovered repo without a manual reload.
      unDiscovered = await onRepoDiscovered((p) => {
        if (!mounted) return;
        dispatchRepoCatalog({ type: "discovered", repo: p });
      });

      // Repo status transitions (active ↔ missing ↔ removed) — backend
      // emits one event per row that changed. Patch allRepos in place
      // so the RepoChip row greys out / restores / drops without a
      // full reload.
      unStatus = await onRepoStatus((payload) => {
        if (!mounted) return;
        dispatchRepoCatalog({ type: "status", payload });
      });

      // Settings can restore an excluded repo while the panel webview stays
      // mounted. Re-pull the canonical catalog so ids/status are authoritative.
      unCatalogChanged = await onRepoCatalogChanged(() => {
        if (!mounted) return;
        setRefreshNonce((nonce) => nonce + 1);
        void listRepos()
          .then((repos) => {
            if (mounted) dispatchRepoCatalog({ type: "replace", repos });
          })
          .catch(() => {});
      });

    })();

    return () => {
      mounted = false;
      unProgress?.();
      unDiscovered?.();
      unStatus?.();
      unCatalogChanged?.();
      unShown?.();
      unTimelineInvalidated?.();
      unFileHistory?.();
      unUpdateModal?.();
      unUpdateNone?.();
      unUpdateIndicator?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ----- all-repos mode: filter facets (authors + per-repo counts) -----
  // The windowed timeline drops the full client-side commit array, so the
  // AuthorsChip list + RepoChip counts come from a backend facet.
  // Refreshed on time-window change and on panel re-summon.
  useEffect(() => {
    if (singleMode) return;
    let cancelled = false;
    const since =
      windowDays === "all"
        ? null
        : Math.floor(Date.now() / 1000) - windowDays * 86_400;
    (async () => {
      try {
        const facets = await listFilterFacets({ since });
        if (cancelled) return;
        setAuthorsAll(facets.authors);
        setRepoCounts(new Map(facets.repos.map((r) => [r.repoId, r.count])));
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, windowDays, refreshNonce]);

  // ----- single-repo mode: saved branch SELECTION -----
  // On repo change, reset selectedBranches to "all" up front so the commits
  // effect never fires with a stale per-repo selection, then restore this
  // repo's saved selection if it has one. Absence of a saved selection ⇒
  // "all", the first-entry default. The branch LIST itself loads in the
  // effect below (kept separate so a refs change can refresh the list
  // without clobbering this selection).
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) {
      setBranches([]);
      setSelectedBranches("all");
      pendingBranchesRef.current = null; // nothing to consume outside a repo
      return;
    }
    // A pending one-shot selection (warp's forced "all", or a history
    // restore's snapshot value) wins over the repo's disk-saved selection —
    // apply it synchronously and skip the disk read entirely.
    const pending = pendingBranchesRef.current;
    pendingBranchesRef.current = null;
    if (pending != null) {
      setSelectedBranches(pending);
      return;
    }
    setSelectedBranches("all");
    let cancelled = false;
    (async () => {
      try {
        const saved = await getBranchSelection(selectedRepoPath);
        if (!cancelled && saved.length > 0) setSelectedBranches(saved);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, selectedRepoPath]);

  // ----- single-repo mode: branch LIST for the chip -----
  // Window-independent (so windowDays is intentionally absent), but DOES
  // depend on refsNonce: when an auto-fetch lands new remote branches — or a
  // local branch op happens — under us, the chip's list refreshes. Kept
  // apart from the selection-reset effect above so this refresh never resets
  // the user's current branch filter.
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) return;
    let cancelled = false;
    (async () => {
      try {
        const bs = await listBranches(selectedRepoPath);
        if (!cancelled) setBranches(bs);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, selectedRepoPath, refsNonce]);

  // Persist the BranchChip selection per repo so it survives across
  // sessions. "all" is stored as an empty list (absence ⇒ "all"), so the
  // first-entry default and an explicit "all" pick collapse to the same
  // thing.
  const handleBranchChange = useCallback(
    (sel: string[] | "all") => {
      // A filter tweak refines the current view (no new history entry) but
      // diverges from any forward history — drop it.
      setViewFwd([]);
      setSelectedBranches(sel);
      if (selectedRepoPath) {
        void saveBranchSelection(selectedRepoPath, sel === "all" ? [] : sel);
      }
    },
    [selectedRepoPath],
  );

  // ----- unified view history: back / forward -----
  // Record the current view before a navigation replaces it. A fresh
  // navigation truncates any forward history (browser semantics).
  const pushView = useCallback(() => {
    setViewBack((b) => [...b, viewRef.current].slice(-VIEW_HISTORY_MAX));
    setViewFwd([]);
  }, []);

  // Restore a snapshot. On a repo CHANGE the branch-selection effect re-runs
  // and would normally load that repo's disk-saved selection — which may not
  // be what this snapshot recorded (the user changed filters since, or a warp
  // had forced "all"). Carry the snapshot's own selection through the
  // one-shot ref so history restores exactly the branches that were on
  // screen. Same-repo restores don't fire that effect, so setSelectedBranches
  // below applies directly. (v.repoPath != null guard: an all-repos restore
  // has no branch scope — the effect's !singleMode arm clears the ref.)
  const applyView = useCallback((v: ViewSnapshot) => {
    if (v.repoPath != null && v.repoPath !== selectedRepoPathRef.current) {
      pendingBranchesRef.current = v.branches;
    }
    setWarpAnchor(null);
    setSelectedRepoPath(v.repoPath);
    setSelectedRepoPaths(v.repoPaths);
    setSelectedBranches(v.branches);
    setWindowDays(v.windowDays);
    setSelectedAuthors(v.authors);
    setFileHistory(v.fileHistory);
    setSearchOpen(v.searchOpen);
    setSearchInput(v.searchInput);
    if (v.searchOpen) setSearchFocusNonce((n) => n + 1);
  }, []);

  const goBack = useCallback(() => {
    if (viewBack.length === 0) return;
    const target = viewBack[viewBack.length - 1];
    setViewFwd((f) => [viewRef.current, ...f].slice(0, VIEW_HISTORY_MAX));
    setViewBack((b) => b.slice(0, -1));
    applyView(target);
  }, [viewBack, applyView]);

  const goForward = useCallback(() => {
    if (viewFwd.length === 0) return;
    const target = viewFwd[0];
    setViewBack((b) => [...b, viewRef.current].slice(-VIEW_HISTORY_MAX));
    setViewFwd((f) => f.slice(1));
    applyView(target);
  }, [viewFwd, applyView]);

  // Leave file-history mode = go back one view (file-history opens always push
  // first, so back lands on the pre-history view). Kept as a named handler for
  // the chip / button / empty-state exit.
  const exitFileHistory = goBack;

  // Warp: push the current view, then land in the commit's single-repo history
  // with the filters auto-corrected — branches "all" (the commit may live on
  // any ref), authors cleared, time window widened just enough to cover it.
  const performWarp = useCallback(
    (c: CommitSummary) => {
      pushView();
      // Leaving any file-history scope — a warp lands in the commit's normal
      // repo history, never a hybrid file-scoped view. (Back still restores
      // the file-history view from the snapshot pushed above.)
      setFileHistory(null);
      // Cross-repo warp: force "all branches" past the disk-saved selection
      // so the target commit is reachable (same-repo warps apply directly).
      if (c.repoPath !== selectedRepoPathRef.current) {
        pendingBranchesRef.current = "all";
      }
      setSelectedRepoPath(c.repoPath);
      setSelectedBranches("all");
      setSelectedAuthors("all");
      setWindowDays((cur) => windowCovering(cur, c.timestamp));
      setSearchOpen(false);
      setSearchCount(null);
      warpNonceRef.current += 1;
      setWarpAnchor({ hash: c.hash, nonce: warpNonceRef.current });
    },
    [pushView],
  );

  // Explicit fetch: await the backend one-shot, then bump refsNonce so the
  // upstream badge re-pulls (fetch-age → fresh, ↑↓ recomputed). The result
  // glyph decays back to idle after a beat. Reads the repo via the live ref
  // so a stale closure can't fetch a previously-viewed repo — and the RESULT
  // only lands if that repo is still the one on screen (a ✓ must never claim
  // freshness for a repo that wasn't fetched).
  const runFetchNow = useCallback(async () => {
    const repo = selectedRepoPathRef.current;
    if (!repo) return;
    // A newer run owns the glyph — a previous run's decay timer must not
    // wipe this run's result early.
    if (fetchNowTimerRef.current != null) {
      window.clearTimeout(fetchNowTimerRef.current);
      fetchNowTimerRef.current = null;
    }
    setFetchNow("fetching");
    let res: string;
    try {
      res = await fetchRepoNow(repo);
    } catch {
      res = "failed";
    }
    const current = selectedRepoPathRef.current;
    if (current == null || !samePath(current, repo)) return; // repo left — drop
    setFetchNow(res === "ok" ? "ok" : res === "busy" ? "busy" : "failed");
    if (res === "ok") setRefsNonce((n) => n + 1);
    fetchNowTimerRef.current = window.setTimeout(() => {
      fetchNowTimerRef.current = null;
      setFetchNow((s) => (s === "fetching" ? s : "idle"));
    }, 2600);
  }, []);

  // Repo switch: whatever the button was showing belonged to the old repo.
  useEffect(() => {
    setFetchNow("idle");
  }, [selectedRepoPath]);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchCount(null);
  }, []);

  // Every visible way into search funnels here — header button, empty-
  // state action, `/` hotkey — so they all focus the input the same way.
  const openSearch = useCallback(() => {
    setOpenChip(null);
    setSearchOpen(true);
    setSearchFocusNonce((n) => n + 1);
  }, []);

  // ----- single-repo mode: upstream status (selection-aware) -----
  // Refetches whenever the repo OR the BranchChip selection changes. Logic:
  //   • "all" or multi-select → HEAD (fall back so the default view shows
  //     something meaningful instead of nothing).
  //   • single LOCAL branch focused → that branch's upstream.
  //   • single REMOTE ref focused → no badge (remote refs have no upstream
  //     of their own in our model).
  useEffect(() => {
    if (!singleMode) {
      setUpstream(null);
      return;
    }
    let cancelled = false;

    let branchParam: string | null = null;
    let skipFetch = false;
    if (selectedBranches !== "all" && selectedBranches.length === 1) {
      const only = selectedBranches[0];
      if (only.startsWith("refs/heads/")) {
        branchParam = only.slice("refs/heads/".length);
      } else if (only.startsWith("refs/remotes/")) {
        skipFetch = true;
      }
    }
    if (skipFetch) {
      setUpstream(null);
      return;
    }

    (async () => {
      try {
        const us = await currentUpstreamStatus(selectedRepoPath!, branchParam);
        if (!cancelled) setUpstream(us);
      } catch {
        if (!cancelled) setUpstream(null);
      }
    })();
    return () => {
      cancelled = true;
    };
    // refsNonce: recompute ahead/behind + fetch-age when refs move under us
    // (an auto-fetch landing, a local commit) — but NOT on a plain summon, so
    // the potentially-pricey graph walk stays off the re-show hot path.
  }, [selectedRepoPath, singleMode, selectedBranches, refsNonce]);

  // ----- single-repo mode: commits (depends on repo, branches, window) -----
  // All three dimensions of the filter must be in the dep list — otherwise
  // changing windowDays or selectedRepoPath while a branch filter is
  // active clobbers the filter (the BranchChip shows "feature" but
  // Timeline silently flips to all-branches). Empty explicit selection
  // returns [] immediately without hitting the backend so "No branches"
  // really means no rows.
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) return;
    let cancelled = false;
    // New view signature → drop the previous view's rows NOW ("Loading
    // commits…"), so a failed fetch can never leave repo A's commits sitting
    // under repo B's header. A same-signature re-pull (refreshNonce) keeps
    // the rows to avoid a flash.
    // File-history mode IGNORES the branch/window lenses, so they must not
    // be part of its signature: the async per-repo saved-branch restore lands
    // AFTER the history rows load, and a signature that included branches
    // would clear them to a permanent "Loading commits…" (the file-history
    // effect has no reason to re-run).
    const sig =
      fileHistory && fileHistory.repoPath === selectedRepoPath
        ? `${selectedRepoPath}|file:${fileHistory.filePath}`
        : `${selectedRepoPath}|${JSON.stringify(selectedBranches)}|${windowDays}`;
    const sameView = commitsSigRef.current === sig;
    if (!sameView) {
      commitsSigRef.current = sig;
      commitsRefsFpRef.current = null; // new view — fingerprint belongs to the old one
      setCommits(null);
      setCommitsError(false);
    }
    // File-history scope wins — its fetch lives in the dedicated effect
    // below (keyed on refsNonce, NOT refreshNonce, so a plain re-summon
    // never repeats the multi-second capped history walk).
    if (fileHistory && fileHistory.repoPath === selectedRepoPath) {
      return;
    }
    if (selectedBranches !== "all" && selectedBranches.length === 0) {
      setCommits([]);
      return;
    }
    (async () => {
      try {
        const branchParam =
          selectedBranches === "all" ? null : selectedBranches;
        // Background re-pull for an UNCHANGED view (a plain re-summon): probe
        // the refs fingerprint first — identical tips mean an identical walk,
        // so skip re-shipping up to 2,000 full commits over IPC for nothing.
        const fp = await repoRefsFingerprint(selectedRepoPath, branchParam);
        if (cancelled) return;
        if (sameView && fp !== "" && fp === commitsRefsFpRef.current) {
          return;
        }
        const cs = await repoCommits(
          selectedRepoPath,
          branchParam,
          toWindowParam(windowDays),
        );
        if (!cancelled) {
          commitsRefsFpRef.current = fp;
          setCommits(cs);
          setCommitsError(false);
        }
      } catch {
        // Repo unopenable (moved, disconnected drive) — an honest error
        // state, never the previous repo's rows. Drop the fingerprint too:
        // if the repo recovers with unchanged refs, the next re-pull must
        // RUN (and clear the error), not fingerprint-skip into a permanently
        // latched "Couldn't open this repo".
        if (!cancelled) {
          commitsRefsFpRef.current = null;
          setCommits([]);
          setCommitsError(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    singleMode,
    selectedRepoPath,
    selectedBranches,
    windowDays,
    refreshNonce,
    fileHistory,
  ]);

  // ----- file-history commits (dedicated effect) -----
  // The capped history walk is the panel's most expensive single query (up
  // to 50k commits × two tree lookups), so it re-runs ONLY when its inputs
  // change: the scope itself, or the repo's refs actually moving (refsNonce).
  // Deliberately NOT refreshNonce — a plain panel re-summon (and the
  // summon's own auto-fetch echo) repeated the multi-second scan for nothing.
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) return;
    if (!fileHistory || fileHistory.repoPath !== selectedRepoPath) return;
    let cancelled = false;
    const { repoPath, filePath } = fileHistory;
    (async () => {
      try {
        const cs = await fetchFileHistory(repoPath, filePath);
        if (!cancelled) setCommits(cs);
      } catch {
        if (!cancelled) setCommits([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, selectedRepoPath, fileHistory, refsNonce]);

  // Manual add via drag-drop / paste. Returns whether the add succeeded
  // so the paste handler can clear the clipboard string only on success.
  // On failure, sets addError to the backend's message ("Not a Git
  // working tree" etc) for inline display.
  const tryAddPath = useCallback(async (rawPath: string): Promise<boolean> => {
    const trimmed = rawPath.trim();
    if (!trimmed) return false;
    try {
      const repo = await explicitAddRepo(trimmed);
      // Add the row directly from the return value. The
      // timeline://repo-discovered event can race with listener
      // registration, so a user-initiated add must not depend on it.
      // The onRepoDiscovered listener dedups by path, so a later event
      // for the same repo is harmless.
      dispatchRepoCatalog({ type: "discovered", repo });
      // Refresh so the new repo carries its real id — the windowed timeline
      // filters by id, not path.
      void listRepos()
        .then((repos) => dispatchRepoCatalog({ type: "replace", repos }))
        .catch(() => {});
      setAddError(null);
      return true;
    } catch (err) {
      setAddError(
        typeof err === "string"
          ? err
          : err instanceof Error
            ? err.message
            : "Failed to add repo",
      );
      window.setTimeout(() => setAddError(null), 4000);
      return false;
    }
  }, []);

  // Panel "sticky" mode — resist blur-dismiss while the user is mid
  // add-repo flow. Two cases: (1) the empty-state screen is up, so the
  // user must reach another window to find a folder; (2) the native
  // folder picker is open and has stolen focus. The backend blur
  // handler skips hide while sticky.
  const emptyState = allRepos.length === 0 && !singleMode;
  const panelSticky = emptyState || dialogOpen;
  useEffect(() => {
    void setPanelSticky(panelSticky);
  }, [panelSticky]);

  // Add a repo via the native folder picker. Sets dialogOpen so the
  // panel stays sticky; also pushes sticky=true synchronously before
  // the picker opens, since the useEffect above races with the picker
  // stealing focus.
  const handleAddRepo = useCallback(async () => {
    setDialogOpen(true);
    await setPanelSticky(true);
    try {
      const selected = await open({
        directory: true,
        multiple: true,
        title: "Add a Git repository",
      });
      if (selected) {
        const list = Array.isArray(selected) ? selected : [selected];
        for (const p of list) {
          await tryAddPath(p);
        }
      }
    } catch {
      // Picker failed to open / plugin error — leave the footer hint
      // and drop/paste paths in place, nothing else to surface.
    } finally {
      setDialogOpen(false);
    }
  }, [tryAddPath]);

  // Tauri drag-drop. Fires on this window's drop zone (the whole panel).
  // We listen for the "drop" variant only — "hover"/"cancel" are just
  // visual cues we'd opt into later. Multi-file drops add each in turn.
  useEffect(() => {
    let un: UnlistenFn | undefined;
    (async () => {
      type DragDrop = { type: string; paths?: string[] };
      un = await getCurrentWindow().listen<DragDrop>("tauri://drag-drop", (e) => {
        if (e.payload.type !== "drop") return;
        const paths = e.payload.paths ?? [];
        for (const p of paths) {
          void tryAddPath(p);
        }
      });
    })();
    return () => un?.();
  }, [tryAddPath]);

  // Paste: only act when the user has clearly pasted a path (starts with
  // a drive letter, slash, or tilde) AND isn't typing into an input/
  // textarea/contenteditable. This way chip search inputs keep working
  // normally — paste only adds repos when there's no other use for it.
  useEffect(() => {
    function onPaste(e: ClipboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (
          tag === "INPUT" ||
          tag === "TEXTAREA" ||
          target.getAttribute("contenteditable") === "true"
        ) {
          return;
        }
      }
      const text = e.clipboardData?.getData("text/plain")?.trim() ?? "";
      if (!text) return;
      // Heuristic: looks like a Windows drive, POSIX absolute, or home-rel path.
      if (!/^([a-zA-Z]:[\\\/]|\/|~[\\\/])/.test(text)) return;
      e.preventDefault();
      void tryAddPath(text);
    }
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [tryAddPath]);

  // ----- `/` (or Ctrl+F): summon the commit search bar -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target) {
        const tag = target.tagName;
        if (
          tag === "INPUT" ||
          tag === "TEXTAREA" ||
          target.getAttribute("contenteditable") === "true"
        ) {
          return;
        }
      }
      const slash =
        e.key === "/" && !e.ctrlKey && !e.metaKey && !e.altKey;
      const ctrlF =
        (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f" && !e.altKey;
      if (!slash && !ctrlF) return;
      if (updateModal) return;
      e.preventDefault();
      openSearch();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [updateModal, openSearch]);

  // ----- view-history keys + ESC cascade -----
  // Alt+←/→ step through the unified view history. Esc: modal → chip →
  // expansion (expansionControlRef rung below) → search → view-back →
  // single-repo → hide panel. The single "view-back" rung subsumes the old
  // warp-return / file-history-exit rungs (both pushed a view on navigate).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // IME composition Enter/Esc (e.g. Hangul in the search input) must
      // never drive the cascade — a cancelled composition is not a "close".
      if (e.isComposing) return;
      // Top layers own the keyboard: the modal closes on Esc, a dropdown
      // handles its own Esc — neither should let view-nav keys churn the
      // timeline underneath.
      if (updateModal) {
        if (e.key === "Escape") {
          setUpdateModal(null);
          e.preventDefault();
        }
        return;
      }
      if (openChip != null) return;
      if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
        e.preventDefault();
        if (e.key === "ArrowLeft") goBack();
        else goForward();
        return;
      }
      if (e.key !== "Escape") return;
      // An open commit expansion is the innermost layer — collapse it first.
      // The mounted timeline registers this control (ExpansionControl); owning
      // the rung here keeps the cascade order deterministic instead of racing
      // window-listener registration order.
      if (expansionControlRef.current?.collapse()) {
        e.preventDefault();
        return;
      }
      // The search input's own Esc is stopPropagation'd; this covers Esc while
      // focus is on the result list.
      if (searchOpen) {
        closeSearch();
        e.preventDefault();
        return;
      }
      // Step back through view history (a warp, file-history open, or repo
      // pick each pushed a view).
      if (viewBack.length > 0) {
        goBack();
        e.preventDefault();
        return;
      }
      // No history left but still scoped to one repo — leave single-repo mode
      // (and retire any warp anchor so a later revisit can't replay it).
      if (singleMode) {
        setWarpAnchor(null);
        setSelectedRepoPath(null);
        e.preventDefault();
        return;
      }
      // Nothing else to close — dismiss the panel itself.
      void dismissPanel();
      e.preventDefault();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    openChip,
    singleMode,
    updateModal,
    searchOpen,
    viewBack,
    viewFwd,
    closeSearch,
    goBack,
    goForward,
  ]);

  // Single-repo mode tallies authors from its (bounded) loaded commits;
  // all-repos mode uses the backend facet (`authorsAll`).
  const authorsSingle: AuthorTally[] = useMemo(() => {
    const m = new Map<string, { count: number; lastActivity: number }>();
    for (const c of commits ?? []) {
      const cur = m.get(c.author);
      if (cur) {
        cur.count += 1;
        if (c.timestamp > cur.lastActivity) cur.lastActivity = c.timestamp;
      } else {
        m.set(c.author, { count: 1, lastActivity: c.timestamp });
      }
    }
    return Array.from(m.entries())
      .map(([name, info]) => ({
        name,
        count: info.count,
        lastActivity: info.lastActivity,
      }))
      .sort((a, b) => b.lastActivity - a.lastActivity);
  }, [commits]);
  const authors = singleMode ? authorsSingle : authorsAll;

  // Single-repo mode only — the all-repos timeline filters server-side.
  // The repo filter doesn't apply here (you're inside one repo already);
  // just narrow the loaded commits by the author selection.
  const filteredCommits = useMemo(() => {
    if (!commits) return null;
    // File history shows every commit that touched the file — the author
    // lens is dimmed/inert in that mode, so don't filter by it.
    if (fileHistory) return commits;
    if (selectedAuthors === "all") return commits;
    const set = new Set(selectedAuthors);
    return commits.filter((c) => set.has(c.author));
  }, [commits, selectedAuthors, fileHistory]);

  // Resolve the multi-repo path filter to backend repo ids for the
  // windowed timeline. id 0 (a just-discovered repo not yet refreshed via
  // listRepos) is dropped; a selection that resolves to no usable ids
  // falls back to "all repos" rather than showing nothing.
  const repoIds = useMemo<number[] | null>(() => {
    if (selectedRepoPaths === "all") return null;
    const byPath = new Map(allRepos.map((r) => [r.path, r.id]));
    const ids = selectedRepoPaths
      .map((p) => byPath.get(p))
      .filter((id): id is number => id != null && id > 0);
    return ids.length > 0 ? ids : null;
  }, [selectedRepoPaths, allRepos]);

  // Search scope: the repo dimension IS respected — single-repo mode
  // searches that repo, all-repos mode follows the RepoChip selection.
  // (A just-discovered repo whose id is still 0 falls back to all repos
  // rather than silently searching nothing.)
  const searchRepoIds = useMemo<number[] | null>(() => {
    if (!singleMode) return repoIds;
    const id = allRepos.find((r) => r.path === selectedRepoPath)?.id;
    return id != null && id > 0 ? [id] : null;
  }, [singleMode, repoIds, allRepos, selectedRepoPath]);

  // Repos the search can actually run over: id is backfilled by listRepos, so
  // id 0 (just-discovered, not yet resolved) can't be searched. The widen
  // affordance only makes sense when more of these exist than the search
  // already covers.
  const usableRepoCount = useMemo(
    () => allRepos.filter((r) => r.id > 0).length,
    [allRepos],
  );

  // A human label for the search's repo scope, derived from the SAME resolved
  // id set the search actually uses (searchRepoIds) — never the raw chip
  // selection — so a zero-result miss names exactly what was searched and the
  // count can't drift from reality. null = the search already spans every repo
  // (whether by "all" or because a narrowed selection's ids haven't resolved),
  // in which case the miss is an honest "not anywhere" with no scope to widen.
  const searchScopeLabel = useMemo<string | null>(() => {
    if (searchRepoIds == null) return null;
    if (searchRepoIds.length === 1) {
      const id = searchRepoIds[0];
      return allRepos.find((r) => r.id === id)?.name ?? "1 repo";
    }
    return `${searchRepoIds.length} repos`;
  }, [searchRepoIds, allRepos]);

  // Only offer "Search all repos" when widening would genuinely broaden the
  // scope — i.e. more usable repos exist than the search currently covers.
  const canWidenSearch =
    searchRepoIds != null && usableRepoCount > searchRepoIds.length;

  // Widen a scoped search to every repo and let it re-run: a real navigation
  // (records the scoped view so back returns to it), dropping single-repo mode
  // and the RepoChip narrowing while keeping the query/search open.
  const widenSearchScope = useCallback(() => {
    pushView();
    setFileHistory(null);
    setSelectedRepoPath(null);
    setSelectedRepoPaths("all");
  }, [pushView]);

  // Time/author changes refine the current view in place — no new history
  // entry, but they diverge from any forward history, so drop it.
  const changeWindowDays = useCallback((v: WindowDays) => {
    setViewFwd([]);
    setWindowDays(v);
  }, []);
  const changeAuthors = useCallback((v: string[] | "all") => {
    setViewFwd([]);
    setSelectedAuthors(v);
  }, []);
  // Picking a repo IS a navigation — record the view we're leaving (and leave
  // any file-history scope). A manual pick also retires the warp anchor:
  // without this, revisiting the warped repo later would scroll-jump and
  // re-pulse the old search hit on remount. Re-picking the CURRENT scope is a
  // no-op (no dead history entry to Back through) — unless it exits a
  // file-history scope, which is a real navigation.
  const changeRepoPath = useCallback(
    (p: string | null) => {
      if (
        p === viewRef.current.repoPath &&
        viewRef.current.fileHistory == null
      ) {
        return;
      }
      pushView();
      setFileHistory(null);
      setWarpAnchor(null);
      setSelectedRepoPath(p);
    },
    [pushView],
  );
  const changeRepoPaths = useCallback(
    (ps: string[] | "all") => {
      if (
        sameRepoSelection(ps, viewRef.current.repoPaths) &&
        viewRef.current.fileHistory == null
      ) {
        return;
      }
      pushView();
      setFileHistory(null);
      setWarpAnchor(null);
      setSelectedRepoPaths(ps);
    },
    [pushView],
  );

  // The RepoChip's "All repos" row resets BOTH scope dimensions — as ONE
  // navigation with ONE history entry (routing it through the two setters
  // above would push twice when both are narrowed).
  const resetRepoScope = useCallback(() => {
    const v = viewRef.current;
    if (v.repoPath == null && v.repoPaths === "all" && v.fileHistory == null) {
      return; // already the widest scope
    }
    pushView();
    setFileHistory(null);
    setWarpAnchor(null);
    setSelectedRepoPath(null);
    setSelectedRepoPaths("all");
  }, [pushView]);

  function togglePin(path: string) {
    setPinnedRepos((prev) => {
      const next = prev.includes(path)
        ? prev.filter((p) => p !== path)
        : [...prev, path];
      void savePinnedRepos(next);
      return next;
    });
  }

  const handleExcludeRepo = useCallback(async (path: string) => {
    // Commit backend state first. A failed command leaves the visible catalog,
    // current scope and navigation history untouched, so no partial rollback
    // path exists.
    await excludeRepo(path);
    dispatchRepoCatalog({
      type: "status",
      payload: { canonicalPath: path, status: "removed" },
    });

    const current = viewRef.current;
    if (current.repoPath === path) {
      setWarpAnchor(null);
      setSelectedRepoPath(null);
    }
    if (current.fileHistory?.repoPath === path) {
      setFileHistory(null);
    }
    if (Array.isArray(current.repoPaths) && current.repoPaths.includes(path)) {
      const remaining = current.repoPaths.filter((repoPath) => repoPath !== path);
      setSelectedRepoPaths(remaining.length === 0 ? "all" : remaining);
    }
    setViewBack((views) => pruneRepoFromViewHistory(views, path));
    setViewFwd((views) => pruneRepoFromViewHistory(views, path));
  }, []);

  // The visible catalog is the single source of truth for the chip count.
  // Scan progress remains a separate presentation state and can never skew it.
  const repoCount = allRepos.length;

  return (
    <main className={"panel" + (singleMode ? " single-mode" : "")}>
      <header
        className="panel-header"
        onPointerDown={startDrag}
        onContextMenu={(e) => {
          const target = e.target as HTMLElement;
          // Chips have their own dropdown behaviour; the close button is
          // its own action. Right-click anywhere else on the header
          // (empty space, drag handle, icon, status, badge) opens the menu.
          if (target.closest(".chip-wrap, .panel-close, .context-menu")) return;
          e.preventDefault();
          setHeaderCtxMenu({
            x: e.clientX,
            y: e.clientY,
            items: [
              {
                label: "Settings…",
                onClick: () => void invoke("open_settings_window"),
              },
            ],
          });
        }}
      >
        <div
          className={
            "panel-header-iconwrap" + (updateVersion ? " has-update" : "")
          }
          title={
            updateVersion
              ? `Update available — v${updateVersion} · click to update`
              : "gitwink"
          }
          role={updateVersion ? "button" : undefined}
          onClick={updateVersion ? openUpdateModal : undefined}
          {...(updateVersion ? { "data-no-drag": true } : {})}
        >
          <img
            src="/icon.png"
            alt="gitwink"
            className="panel-header-icon"
            draggable={false}
          />
          {updateVersion && (
            <span className="panel-header-update-badge" aria-hidden="true">
              !
            </span>
          )}
        </div>
        <div className="header-chips">
          {(viewBack.length > 0 || viewFwd.length > 0) && (
            <span className="view-nav" role="group" aria-label="View history">
              <button
                type="button"
                className="view-nav-btn"
                onClick={goBack}
                disabled={viewBack.length === 0}
                title="Back (Alt+←)"
                aria-label="Back"
              >
                ‹
              </button>
              <button
                type="button"
                className="view-nav-btn"
                onClick={goForward}
                disabled={viewFwd.length === 0}
                title="Forward (Alt+→)"
                aria-label="Forward"
              >
                ›
              </button>
            </span>
          )}
          {fileHistory && (
            <button
              type="button"
              className="filehist-chip"
              onClick={exitFileHistory}
              title={`History of ${fileHistory.filePath} — click to exit (Esc)`}
            >
              <span className="filehist-chip-icon" aria-hidden="true">
                🕘
              </span>
              <span className="filehist-chip-name">
                {fileHistory.filePath.split("/").pop()}
              </span>
              <span className="filehist-chip-x" aria-hidden="true">
                ✕
              </span>
            </button>
          )}
          <RepoChip
            open={openChip === "repo"}
            onToggle={() => setOpenChip(openChip === "repo" ? null : "repo")}
            onClose={() => setOpenChip(null)}
            repos={allRepos}
            repoCounts={repoCounts}
            pinned={pinnedRepos}
            selectedPath={selectedRepoPath}
            selectedPaths={selectedRepoPaths}
            onSelect={changeRepoPath}
            onSelectMulti={changeRepoPaths}
            onSelectAll={resetRepoScope}
            onTogglePin={togglePin}
            onExclude={handleExcludeRepo}
            totalRepoCount={repoCount}
          />
          {singleMode && (
            <span
              className={
                "chip-slot" +
                (searching || fileHistory ? " chip-dimmed" : "")
              }
              title={
                fileHistory
                  ? "Not applied in file history"
                  : searching
                    ? "Not applied while searching"
                    : undefined
              }
            >
              <BranchChip
                open={openChip === "branch"}
                onToggle={() =>
                  setOpenChip(openChip === "branch" ? null : "branch")
                }
                onClose={() => setOpenChip(null)}
                repoPath={selectedRepoPath ?? ""}
                branches={branches}
                selected={selectedBranches}
                onChange={handleBranchChange}
              />
            </span>
          )}
          {singleMode && upstream && !fileHistory && (
            <UpstreamBadge status={upstream} />
          )}
          {/* Fetch button is NOT gated on the upstream badge — a local-only
              branch with no upstream is exactly where a manual fetch is
              needed (origin absence just fails fast → "!"). */}
          {singleMode && !fileHistory && (
            <button
              type="button"
              className={
                "fetch-now-btn" +
                (fetchNow === "fetching" ? " fetching" : "") +
                (fetchNow === "failed" || fetchNow === "busy" ? " failed" : "")
              }
              disabled={fetchNow === "fetching"}
              onClick={() => void runFetchNow()}
              aria-label={
                fetchNow === "fetching"
                  ? "Fetching origin"
                  : fetchNow === "ok"
                    ? "Fetched — origin refs are current"
                    : fetchNow === "busy"
                      ? "Another fetch is running"
                      : fetchNow === "failed"
                        ? "Fetch failed"
                        : "Fetch origin now"
              }
              title={
                fetchNow === "fetching"
                  ? "Fetching origin…"
                  : fetchNow === "ok"
                    ? "Fetched ✓ — origin refs are current"
                    : fetchNow === "busy"
                      ? "Another fetch is running — try again in a moment"
                      : fetchNow === "failed"
                        ? "Fetch failed (offline? no origin? another fetch mid-flight?) — counts reflect the last successful fetch"
                        : "Fetch origin now — refresh against the remote (read-only; never merges or pushes)"
              }
            >
              {fetchNow === "ok" ? "✓" : fetchNow === "failed" || fetchNow === "busy" ? "!" : "↻"}
            </button>
          )}
          {/* Time + author chips are view lenses — an active search
              bypasses them (dimmed to say so). The repo scope above
              stays live. */}
          <span
            className={
              "chip-slot" + (searching || fileHistory ? " chip-dimmed" : "")
            }
            title={
              fileHistory
                ? "Not applied in file history"
                : searching
                  ? "Not applied while searching"
                  : undefined
            }
          >
            <TimeRangeChip
              open={openChip === "time"}
              onToggle={() => setOpenChip(openChip === "time" ? null : "time")}
              onClose={() => setOpenChip(null)}
              value={windowDays}
              onChange={changeWindowDays}
            />
          </span>
          <span
            className={
              "chip-slot" + (searching || fileHistory ? " chip-dimmed" : "")
            }
            title={
              fileHistory
                ? "Not applied in file history"
                : searching
                  ? "Not applied while searching"
                  : undefined
            }
          >
            <AuthorsChip
              open={openChip === "authors"}
              onToggle={() =>
                setOpenChip(openChip === "authors" ? null : "authors")
              }
              onClose={() => setOpenChip(null)}
              authors={authors}
              selected={selectedAuthors}
              onChange={changeAuthors}
            />
          </span>
        </div>
        <div className="panel-drag-handle" />
        {scanning && <span className="panel-status">Scanning…</span>}
        {upToDate && !scanning && (
          <span className="panel-status">✓ Up to date</span>
        )}
        {/* The always-visible way into commit search — the `/` shortcut
            alone is invisible to anyone who hasn't read the docs. */}
        <button
          type="button"
          className={"panel-search-btn" + (searchOpen ? " active" : "")}
          onClick={() => (searchOpen ? closeSearch() : openSearch())}
          title="Search commits — message, author, SHA (/)"
        >
          ⌕
        </button>
        <button
          type="button"
          className={"panel-pin" + (pinned ? " pinned" : "")}
          onClick={async () => {
            const next = !pinned;
            try {
              // set_panel_pinned now returns a Result — refuse to flip
              // the pin glyph if the disk persist fails, so the UI and
              // the next launch agree (GPT Pro review A3 caveat).
              await invoke("set_panel_pinned", { pinned: next });
            } catch (err) {
              // eslint-disable-next-line no-console
              console.error("[gitwink] set_panel_pinned failed", err);
              return;
            }
            // Mirror to lib/settings + emit to every window so the pin
            // glyph flips immediately (rather than waiting for the next
            // get_settings round-trip).
            void broadcastSettings({
              ...getCurrentSettings(),
              panelPinned: next,
            });
          }}
          title={
            pinned
              ? "Unpin — return to glance mode (auto-hides on blur, always-on-top)"
              : "Pin — keep open while clicking elsewhere; not always-on-top. Summon via tray / hotkey."
          }
        >
          📌
        </button>
        <button
          type="button"
          className="panel-close"
          onClick={() => void dismissPanel()}
          title="Close (Esc) — closes diff window too"
        >
          ✕
        </button>
      </header>
      {searchOpen && (
        <SearchBar
          value={searchInput}
          count={searching ? searchCount : null}
          focusNonce={searchFocusNonce}
          onChange={setSearchInput}
          onClose={closeSearch}
          onMove={(d) => searchControlRef.current?.moveSelection(d)}
          onActivate={() => searchControlRef.current?.activateSelected()}
        />
      )}
      {showAutoFetchNotice && (
        <div className="autofetch-notice" role="status">
          <span className="autofetch-notice-text">
            gitwink now fetches the open repo from <code>origin</code> when you
            view it, so a teammate's pushed commits show up. It only updates the
            remote-tracking mirror — it never merges, pushes, or changes your
            repo.
          </span>
          <span className="autofetch-notice-actions">
            <button
              type="button"
              className="autofetch-notice-link"
              onClick={() => void invoke("open_settings_window")}
            >
              Settings
            </button>
            <button
              type="button"
              className="autofetch-notice-dismiss"
              onClick={dismissAutoFetchNotice}
            >
              Got it
            </button>
          </span>
        </div>
      )}
      <section className="panel-body">
        {!firstRunTipSeen && !searching && !fileHistory && allRepos.length > 0 && (
          <div className="firstrun-tip">
            <span className="firstrun-tip-text">
              <kbd>/</kbd> search · click a commit to expand · the 🕘 on a file
              opens its history · right-click to copy for AI
            </span>
            <button
              type="button"
              className="firstrun-tip-dismiss"
              aria-label="Dismiss tip"
              title="Got it"
              onClick={dismissFirstRunTip}
            >
              ✕
            </button>
          </div>
        )}
        {allRepos.length === 0 && !singleMode ? (
          <EmptyDropPanel
            scanning={scanning}
            addError={addError}
            onBrowse={() => void handleAddRepo()}
          />
        ) : (
          <>
            {searching && (
              // Active search: the timeline body IS the result list — the
              // same windowed machinery with the query as one more filter
              // (time/author bypassed, repo scope respected). Enter / ↗
              // warps into the commit's single-repo history.
              <TimelineWindowed
                key="search-results"
                repoIds={searchRepoIds}
                authors={null}
                windowDays={null}
                refreshNonce={refreshNonce}
                query={searchQuery}
                skipRefill
                searchMode
                onWarp={performWarp}
                searchControlRef={searchControlRef}
                onResultCount={setSearchCount}
                onSelectRepo={changeRepoPath}
                searchScopeLabel={searchScopeLabel}
                onWidenSearch={canWidenSearch ? widenSearchScope : undefined}
                expansionControlRef={expansionControlRef}
              />
            )}
            {!searching && singleMode && (
          filteredCommits == null ? (
            <p className="panel-empty">Loading commits…</p>
          ) : commitsError ? (
            // The repo itself couldn't be opened — say so instead of showing
            // an empty (or worse, a stale) timeline under its name.
            <div className="panel-empty">
              <p className="panel-empty-line">
                Couldn't open this repo — it may have moved or be on a
                disconnected drive.
              </p>
              <p className="panel-empty-actions">
                <button
                  type="button"
                  className="panel-empty-action"
                  onClick={() => changeRepoPath(null)}
                >
                  Back to all repos
                </button>
              </p>
            </div>
          ) : filteredCommits.length === 0 ? (
            fileHistory ? (
              // File-history mode owns its empty state — the time/author/
              // branch buttons below would be dead ends here. Name the file
              // and offer the one real way out.
              <div className="panel-empty">
                <p className="panel-empty-line">
                  No history found for{" "}
                  <code className="panel-empty-file">
                    {fileHistory.filePath.split("/").pop()}
                  </code>{" "}
                  in this repo.
                </p>
                <p className="panel-empty-sub">Renames aren't followed yet.</p>
                <p className="panel-empty-actions">
                  <button
                    type="button"
                    className="panel-empty-action"
                    onClick={exitFileHistory}
                  >
                    Exit file history
                  </button>
                </p>
              </div>
            ) : (
            // Filter-aware empty state — name the filter hiding the
            // commits and offer the one-click way out, instead of making
            // the user debug the time/author/branch filter stack.
            <div className="panel-empty">
              <p className="panel-empty-line">
                {windowDays !== "all"
                  ? windowDays === 1
                    ? "No commits in the last 24 hours."
                    : `No commits in the last ${windowDays} days.`
                  : "No commits match."}
                {selectedAuthors !== "all" &&
                  ` Filtered to ${selectedAuthors.length} author${selectedAuthors.length === 1 ? "" : "s"}.`}
                {selectedBranches !== "all" &&
                  ` Filtered to ${selectedBranches.length} branch${selectedBranches.length === 1 ? "" : "es"}.`}
              </p>
              <p className="panel-empty-actions">
                {windowDays !== "all" && (
                  <button
                    type="button"
                    className="panel-empty-action"
                    onClick={() => changeWindowDays("all")}
                  >
                    Show all time
                  </button>
                )}
                {selectedAuthors !== "all" && (
                  <button
                    type="button"
                    className="panel-empty-action"
                    onClick={() => changeAuthors("all")}
                  >
                    Clear author filter
                  </button>
                )}
                {selectedBranches !== "all" && (
                  <button
                    type="button"
                    className="panel-empty-action"
                    onClick={() => handleBranchChange("all")}
                  >
                    All branches
                  </button>
                )}
                <button
                  type="button"
                  className="panel-empty-action"
                  onClick={openSearch}
                  title="Search commits — message, author, SHA (/)"
                >
                  Search commits
                </button>
              </p>
            </div>
            )
          ) : (
            <Timeline
              key={`single:${selectedRepoPath}`}
              commits={filteredCommits}
              allCommits={commits ?? undefined}
              branches={branches}
              resetKey={`${fileHistory ? `file:${fileHistory.filePath}` : ""}${JSON.stringify(selectedBranches)}|${windowDays}|${JSON.stringify(selectedAuthors)}`}
              anchor={warpAnchor}
              linear={fileHistory != null}
              expansionControlRef={expansionControlRef}
            />
          )
            )}
            {/* The browse all-repos timeline stays MOUNTED while search or
                single-repo mode sits on top: hiding instead of unmounting
                preserves its scroll/selection/expansion across a search
                bounce or a drill-in → Esc, and skips the remount reload.
                `inactive` hands the keyboard + Esc rung to the visible view. */}
            <div
              className={
                searching || singleMode ? "browse-host-hidden" : "browse-host"
              }
            >
              <TimelineWindowed
                repoIds={repoIds}
                authors={selectedAuthors === "all" ? null : selectedAuthors}
                windowDays={toWindowParam(windowDays)}
                refreshNonce={refreshNonce}
                onSelectRepo={changeRepoPath}
                onShowAllTime={() => changeWindowDays("all")}
                onClearAuthors={() => changeAuthors("all")}
                onOpenSearch={openSearch}
                expansionControlRef={expansionControlRef}
                inactive={searching || singleMode}
              />
            </div>
          </>
        )}
        {allRepos.length > 0 && (
          <div className="panel-footer-hint">
            <button
              type="button"
              className="add-repo-btn"
              onClick={() => void handleAddRepo()}
            >
              + Add repo…
            </button>
            <span
              className="panel-footer-hint-text"
              title="Copy a repo folder's path in your file manager, then paste it here"
            >
              or paste a path
            </span>
            {addError && <span className="panel-footer-hint-error"> · {addError}</span>}
          </div>
        )}
      </section>
      {updateModal && (
        <UpdateModal
          state={updateModal}
          onClose={() => setUpdateModal(null)}
        />
      )}
      {headerCtxMenu && (
        <ContextMenu
          items={headerCtxMenu.items}
          x={headerCtxMenu.x}
          y={headerCtxMenu.y}
          onClose={() => setHeaderCtxMenu(null)}
        />
      )}
    </main>
  );
}

interface EmptyDropPanelProps {
  scanning: boolean;
  addError: string | null;
  onBrowse: () => void;
}

/** First-paint state for a fresh PC where no repos are cached AND the
 * background scan hasn't found anything yet (no VS Code recents, no
 * git config hints, etc). Shows a big drop target as the *primary* UI
 * rather than a blank "Scanning…" screen — the explicit-add path is a
 * first-class flow, not a hidden escape hatch. The panel is sticky
 * (resists blur-dismiss) while this screen is up, so the user can
 * reach a file-manager window to drag a folder back without the panel
 * closing. */
function EmptyDropPanel({ scanning, addError, onBrowse }: EmptyDropPanelProps) {
  return (
    <div className="empty-drop">
      <div className="empty-drop-icon" aria-hidden="true">
        📂
      </div>
      <div className="empty-drop-title">Drop a repo folder here</div>
      <div className="empty-drop-sub">or paste a path (Ctrl+V / Cmd+V)</div>
      <button
        type="button"
        className="add-repo-btn empty-drop-btn"
        onClick={onBrowse}
      >
        Browse for a folder…
      </button>
      {scanning && (
        <div className="empty-drop-status">Scanning for repos…</div>
      )}
      {addError && <div className="empty-drop-error">{addError}</div>}
    </div>
  );
}

export default App;
