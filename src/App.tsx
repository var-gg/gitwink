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
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useLingui } from "@lingui/react/macro";

import { ContextMenu, type MenuItem } from "./components/ContextMenu";
import { PanelChromeRow } from "./components/header/PanelChromeRow";
import { PanelFilterRow } from "./components/panel/PanelFilterRow";
import {
  EmptyDropPanel,
  FirstRunTip,
  PanelFooter,
} from "./components/panel/PanelOnboarding";
import {
  PanelNotices,
  type ControlNotice,
} from "./components/panel/PanelNotices";
import { SingleRepoView } from "./components/panel/SingleRepoView";
import { SEARCH_RESULTS_LIST_ID } from "./components/searchAccessibility";
import {
  TimelineWindowed,
  type ExpansionControl,
  type SearchControl,
} from "./components/TimelineWindowed";
import { WINDOW_DAY_PRESETS } from "./components/TimeRangeChip";
import {
  ackAutoFetchNotice,
  cancelFileHistory,
  currentUpstreamStatus,
  dismissPanel,
  excludeRepo,
  fetchRepoNow,
  fileHistory as fetchFileHistory,
  getBranchSelection,
  getHistoryIndexStatus,
  repoRefsFingerprint,
  getPinnedRepos,
  getScanState,
  listBranches,
  listRepos,
  onFileHistoryOpen,
  onOrchestratorProgress,
  onPanelShown,
  onRepoCatalogChanged,
  onRepoDiscovered,
  onRepoStatus,
  onTimelineInvalidated,
  onUpdateIndicator,
  onUpdateShowModal,
  repoCommits,
  releaseHistoryIndex,
  requestHistoryIndex,
  setBranchSelection as saveBranchSelection,
  setPinnedRepos as savePinnedRepos,
  updateGetState,
  updateRefreshIndicator,
  type ControlDelivery,
} from "./lib/ipc";
import {
  broadcastSettings,
  getCurrentSettings,
  usePanelPinned,
} from "./lib/settings";
import {
  repoCatalogReducer,
  resolveRepoIds,
  sameCanonicalPath,
} from "./lib/repoCatalog";
import {
  pruneRepoFromViewHistory,
  type ViewSnapshot,
} from "./lib/viewHistory";
import { timelineSnapshotFilterKey } from "./lib/useTimelineWindow";
import { isSearchResultCurrent } from "./lib/searchResolution";
import { useContextualFacets } from "./hooks/useContextualFacets";
import { useRepositoryIntake } from "./hooks/useRepositoryIntake";
import { mergeAnchoredCommit } from "./lib/controlHandoff";
import { useControlDelivery } from "./lib/useControlDelivery";
import { shouldPresentUpdate } from "./lib/updatePolicy";
import { useUpdateCheckNotice } from "./lib/useUpdateCheckNotice";
import {
  filterRollingWindow,
  nextRollingWindowExpiry,
} from "./lib/rollingWindow";
import {
  canReuseRepoCommitsSnapshot,
  reusableRepoCommitsFingerprint,
} from "./lib/repoCommitsReuse";
import { UpdateModal } from "./components/UpdateModal";
import { dismissContextMenus } from "./components/overlayLifecycle";
import { panelMessages } from "./i18n/messages/panel";
import { translate } from "./i18n/translate";
import type {
  AuthorTally,
  BranchInfo,
  CommitSummary,
  FileHistoryResult,
  HistoryIndexStatus,
  UpdateStatePayload,
  UpstreamStatus,
  WindowDays,
} from "./types";
import "./styles.css";

function visibleUpdateModalState(
  state: UpdateStatePayload,
): UpdateStatePayload | null {
  return shouldPresentUpdate(state) ? state : null;
}

function startDrag(e: React.PointerEvent<HTMLElement>) {
  if (e.button !== 0) return;
  const target = e.target as HTMLElement | null;
  // Don't start a drag if the press landed on a clickable control or
  // inside an open chip dropdown (incl. its scrollbars / inputs / pin
  // buttons). The user is interacting with the dropdown, not the window.
  if (target?.closest("button, input, .chip-dropdown, [data-no-drag]")) return;
  void getCurrentWindow().startDragging();
}

function toWindowParam(w: WindowDays): number | null {
  return w === "all" ? null : (w as number);
}

const FILE_HISTORY_FLOW_ID = "panel-file-history";
let nextHistoryDemandId = 0;
const HISTORY_DEMAND_SEQUENCE_KEY = "gitwink.historyDemandSequence";

function allocateHistoryDemandId(): number {
  let persisted = 0;
  try {
    persisted = Number(sessionStorage.getItem(HISTORY_DEMAND_SEQUENCE_KEY)) || 0;
  } catch {
    // Sandboxed test/webview storage can be unavailable; the in-memory and
    // wall-clock components still keep this process monotonic.
  }
  nextHistoryDemandId = Math.max(
    nextHistoryDemandId + 1,
    persisted + 1,
    Date.now() * 1_000,
  );
  try {
    sessionStorage.setItem(
      HISTORY_DEMAND_SEQUENCE_KEY,
      String(nextHistoryDemandId),
    );
  } catch {}
  return nextHistoryDemandId;
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
  const { i18n } = useLingui();
  const [scanning, setScanning] = useState(false);
  const [commits, setCommits] = useState<CommitSummary[] | null>(null);
  const [repoCommitsCoverage, setRepoCommitsCoverage] = useState<{
    truncated: boolean;
    incomplete: boolean;
    limit: number;
    shallow: boolean;
  } | null>(null);
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
  const [windowDays, setWindowDays] = useState<WindowDays>(7);
  const [singleRepoWindowNow, setSingleRepoWindowNow] = useState(() =>
    Math.floor(Date.now() / 1000),
  );
  const [selectedRepoPath, setSelectedRepoPath] = useState<string | null>(null);
  const [selectedRepoPaths, setSelectedRepoPaths] = useState<string[] | "all">(
    "all",
  );
  const [selectedAuthors, setSelectedAuthors] = useState<string[] | "all">(
    "all",
  );

  // Single-repo mode state.
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [branchesLoading, setBranchesLoading] = useState(false);
  const branchesRepoPathRef = useRef<string | null>(null);
  const branchHydrationEpochRef = useRef(0);
  const [selectedBranches, setSelectedBranches] = useState<string[] | "all">(
    "all",
  );
  const [upstream, setUpstream] = useState<UpstreamStatus | null>(null);

  // Bumped each time the panel is summoned — the windowed timeline, the
  // author facet, and the single-repo commits effect all depend on it, so
  // re-showing the panel re-pulls (covering anything the watcher missed).
  const [refreshNonce, setRefreshNonce] = useState(0);
  // Cache-only history publication refresh. Unlike refreshNonce this never
  // authorizes another legacy Git fleet walk.
  const [historyRefreshNonce, setHistoryRefreshNonce] = useState(0);

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
  const [fileHistoryCoverage, setFileHistoryCoverage] = useState<Omit<
    FileHistoryResult,
    "rows"
  > | null>(null);
  const [fileHistoryError, setFileHistoryError] = useState(false);
  const fileHistoryOperationRef = useRef(0);

  // The exact target stays injected until the user navigates away, even when
  // it sits outside the normal bounded single-repo snapshot.
  const [anchoredCommit, setAnchoredCommit] =
    useState<CommitSummary | null>(null);
  const [controlNotice, setControlNotice] =
    useState<ControlNotice | null>(null);

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
  const [resolvedSearchQuery, setResolvedSearchQuery] = useState<string | null>(
    null,
  );
  const [resolvedSearchFilterKey, setResolvedSearchFilterKey] = useState<
    string | null
  >(null);
  const [resolvedSearchEpoch, setResolvedSearchEpoch] = useState<number | null>(
    null,
  );
  const [searchCount, setSearchCount] = useState<number | null>(null);
  const [searchFocusNonce, setSearchFocusNonce] = useState(0);
  const [historyIndexStatus, setHistoryIndexStatus] =
    useState<HistoryIndexStatus | null>(null);
  const [historyIndexError, setHistoryIndexError] = useState(false);
  const [historyIndexRetryNonce, setHistoryIndexRetryNonce] = useState(0);
  const [searchActiveDescendant, setSearchActiveDescendant] = useState<
    string | null
  >(null);
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
    expand?: boolean;
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
    anchoredCommit: null,
    anchorExpand: false,
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
    anchoredCommit,
    anchorExpand: warpAnchor?.expand ?? false,
  };
  // One-shot branch selection for the NEXT repo change, consumed by the
  // branch-selection effect INSTEAD of the repo's disk-saved selection.
  // Two writers: a warp forces "all" (the target commit may not be reachable
  // from the saved filter), and a history restore (applyView) carries the
  // snapshot's own selection — so Back/Forward re-lands on the branches the
  // user was actually looking at, not whatever the repo's latest save says.
  const pendingBranchesRef = useRef<string[] | "all" | null>(null);

  useEffect(() => {
    if (searchInput.trim() === "") {
      setSearchQuery("");
      setResolvedSearchQuery("");
      return;
    }
    const timer = window.setTimeout(() => setSearchQuery(searchInput), 150);
    return () => window.clearTimeout(timer);
  }, [searchInput]);

  // The visible input owns the body immediately. Waiting for the debounced
  // query would leave the normal browse rows clickable under a search header
  // for 150 ms after the first keystroke.
  const searching = searchOpen && searchInput.trim().length > 0;

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
  const updateCheckNotice = useUpdateCheckNotice();

  // Open the update modal from the header badge — same path as the tray
  // "Update available" item.
  const openUpdateModal = useCallback(() => {
    void updateGetState()
      .then((st) => setUpdateModal(visibleUpdateModalState(st)))
      .catch(() => {});
  }, []);

  // The updater is a true modal layer. Retire every header-owned overlay as
  // soon as it opens so no dropdown/context menu can keep keyboard focus or
  // reappear stale when the modal closes. Context menus owned deeper in the
  // timelines also listen for UpdateModal's global modal-open signal.
  useEffect(() => {
    if (!updateModal) return;
    setOpenChip(null);
    setHeaderCtxMenu(null);
  }, [updateModal]);

  const singleMode = selectedRepoPath != null;
  const emptyState = allRepos.length === 0 && !singleMode;
  const { addError, browse: browseForRepo } = useRepositoryIntake({
    emptyState,
    dispatchCatalog: dispatchRepoCatalog,
  });

  // Resolve the multi-repo scope once for the timeline, search and contextual
  // facets. `[]` is an honest unresolved/empty scope; only an explicit
  // `"all"` selection becomes null. Never silently widen a narrow chip state.
  const repoIds = useMemo(
    () => resolveRepoIds(selectedRepoPaths, allRepos),
    [selectedRepoPaths, allRepos],
  );
  const {
    authors: authorsAll,
    repoCounts,
    authorsLoading,
    repoCountsLoading,
    error: facetsError,
    retry: retryFacets,
    acceptSnapshot: acceptBrowseSnapshot,
  } = useContextualFacets({
    singleMode,
    repoIds,
    selectedAuthors,
    windowDays,
  });
  // Search scope: single-repo mode searches that repo; all-repos mode follows
  // the RepoChip. An unresolved id is an empty scope, never permission to
  // silently widen to every repository.
  const searchRepoIds = useMemo<number[] | null>(() => {
    if (!singleMode) return repoIds;
    const id = allRepos.find((r) => r.path === selectedRepoPath)?.id;
    return id != null && id > 0 ? [id] : [];
  }, [singleMode, repoIds, allRepos, selectedRepoPath]);
  // Opening an empty search box must be free. Once a debounced query exists,
  // paint the rolling-cache result first and start progressive expansion a
  // beat later so the background writer cannot convoy the user's first COUNT
  // + page reads on SQLite.
  const searchHistoryDemanded =
    searchOpen && searchQuery.trim().length > 0;
  const historyDemanded =
    searchHistoryDemanded || (!singleMode && windowDays === "all");
  const historyRepoIds = searchOpen ? searchRepoIds : repoIds;
  // Include catalog availability in the identity, not only selected ids. A
  // missing drive becoming active must immediately re-demand its history;
  // likewise a newly discovered/offline repo must stop an all-scope view from
  // falsely retaining the previous scope's completeness claim.
  const historyScopeKey = useMemo(() => {
    const selected =
      historyRepoIds == null ? null : new Set<number>(historyRepoIds);
    const repos = allRepos
      .filter((repo) => selected == null || selected.has(repo.id))
      .map((repo) => `${repo.id}:${repo.status}`)
      .sort()
      .join("|");
    return `${selected == null ? "all" : "ids"}:${repos}`;
  }, [allRepos, historyRepoIds]);

  // Complete history is deliberately absent from startup. Search and the
  // all-time fleet view request only their exact repo scope, then poll one
  // tiny aggregate while the single coalescing worker publishes repositories
  // atomically. Existing published snapshots stay usable during refresh.
  useEffect(() => {
    if (
      !historyDemanded ||
      (Array.isArray(historyRepoIds) && historyRepoIds.length === 0)
    ) {
      setHistoryIndexStatus(null);
      setHistoryIndexError(false);
      return;
    }

    let disposed = false;
    let timer: number | undefined;
    const demandId = allocateHistoryDemandId();
    let lastGeneration: number | null = null;
    let lastPublishedRepos: number | null = null;
    setHistoryIndexStatus(null);
    setHistoryIndexError(false);

    const schedulePoll = (delay: number) => {
      timer = window.setTimeout(async () => {
        try {
          accept(await getHistoryIndexStatus(historyRepoIds));
        } catch {
          if (!disposed) {
            setHistoryIndexError(true);
            schedulePoll(1_500);
          }
        }
      }, delay);
    };

    const accept = (status: HistoryIndexStatus) => {
      if (disposed) return;
      if (
        lastGeneration == null ||
        status.generation !== lastGeneration ||
        status.publishedRepos !== lastPublishedRepos
      ) {
        setHistoryRefreshNonce((nonce) => nonce + 1);
      }
      lastGeneration = status.generation;
      lastPublishedRepos = status.publishedRepos;
      setHistoryIndexStatus(status);
      setHistoryIndexError(false);
      // Keep a low-rate settled watch while the demanding view is open: a
      // later watcher rewrite can move coverage back to stale/indexing.
      schedulePoll(status.settled ? 2_000 : status.indexingRepos > 0 ? 250 : 500);
    };

    const request = () => {
      void requestHistoryIndex(historyRepoIds, demandId)
        .then(accept)
        .catch(() => {
          if (!disposed) {
            setHistoryIndexError(true);
            timer = window.setTimeout(request, 1_500);
          }
        });
    };
    if (searchHistoryDemanded) {
      // Let the exact rolling-cache result paint first and give the one-time
      // cold FTS catch-up an uncontended head start. Full-history Git I/O is
      // valuable, but never at the expense of the keystroke-to-result path.
      timer = window.setTimeout(request, 650);
    } else {
      request();
    }

    return () => {
      disposed = true;
      if (timer != null) window.clearTimeout(timer);
      void releaseHistoryIndex(demandId).catch(() => {});
    };
    // historyScopeKey is the stable value identity; array instances can be
    // recreated by catalog reducers without changing the requested scope.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    historyDemanded,
    historyScopeKey,
    historyIndexRetryNonce,
    searchHistoryDemanded,
  ]);
  const searchFilterKey = timelineSnapshotFilterKey(searchRepoIds, null, null);
  const searchResolutionEpoch = refreshNonce + historyRefreshNonce;
  const searchResolved =
    resolvedSearchEpoch === searchResolutionEpoch &&
    isSearchResultCurrent(
      searchInput,
      searchQuery,
      resolvedSearchQuery,
      searchFilterKey,
      resolvedSearchFilterKey,
    );
  const clearResolvedSearchSnapshot = useCallback(() => {
    setResolvedSearchQuery(null);
    setResolvedSearchFilterKey(null);
    setResolvedSearchEpoch(null);
    setSearchActiveDescendant(null);
  }, []);

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
        if (p.repoPath && !sameCanonicalPath(p.repoPath, cur)) return;
        setRefreshNonce((n) => n + 1); // commits re-pull
        setRefsNonce((n) => n + 1); // branch list + upstream badge re-eval
      });

      // A diff window asked to show a file's history — enter single-repo mode
      // for that repo and scope the timeline to the file. The commits effect
      // sees `fileHistory` and fetches the file's history instead.
      unFileHistory = await onFileHistoryOpen((p) => {
        if (!mounted) return;
        setControlNotice(null);
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
        setWarpAnchor(null);
        setAnchoredCommit(null);
        setSelectedRepoPath(p.repoPath);
        setFileHistoryCoverage(null);
        setFileHistoryError(false);
        setFileHistory({ repoPath: p.repoPath, filePath: p.filePath });
        setRefreshNonce((n) => n + 1);
      });

      // Updater: backend asks the panel to surface the modal (tray
      // "Update available" item, a manual check hit, or a Scoop install).
      unUpdateModal = await onUpdateShowModal(async () => {
        try {
          const st = await updateGetState();
          if (mounted) setUpdateModal(visibleUpdateModalState(st));
        } catch {}
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
        void getPinnedRepos()
          .then((pins) => {
            if (mounted) setPinnedRepos(pins);
          })
          .catch(() => {});
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
      unUpdateIndicator?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ----- single-repo mode: saved branch SELECTION -----
  // On repo change, reset selectedBranches to "all" up front so the commits
  // effect never fires with a stale per-repo selection, then restore this
  // repo's saved selection if it has one. Absence of a saved selection ⇒
  // "all", the first-entry default. The branch LIST itself loads in the
  // effect below (kept separate so a refs change can refresh the list
  // without clobbering this selection).
  useEffect(() => {
    const hydrationEpoch = ++branchHydrationEpochRef.current;
    if (!singleMode || !selectedRepoPath) {
      setBranches([]);
      setBranchesLoading(false);
      branchesRepoPathRef.current = null;
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
        if (
          !cancelled &&
          branchHydrationEpochRef.current === hydrationEpoch &&
          saved.length > 0
        ) {
          setSelectedBranches(saved);
        }
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
    const repoChanged = branchesRepoPathRef.current !== selectedRepoPath;
    branchesRepoPathRef.current = selectedRepoPath;
    if (repoChanged) setBranches([]);
    setBranchesLoading(true);
    (async () => {
      try {
        const bs = await listBranches(selectedRepoPath);
        if (!cancelled) setBranches(bs);
      } catch {
        if (!cancelled && repoChanged) setBranches([]);
      } finally {
        if (!cancelled) setBranchesLoading(false);
      }
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
      setWarpAnchor(null);
      setAnchoredCommit(null);
      setControlNotice(null);
      // A manual choice wins over an in-flight saved-selection hydrate.
      branchHydrationEpochRef.current += 1;
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
    branchHydrationEpochRef.current += 1;
    if (v.repoPath != null && v.repoPath !== selectedRepoPathRef.current) {
      pendingBranchesRef.current = v.branches;
    }
    setAnchoredCommit(v.anchoredCommit);
    if (v.anchoredCommit) {
      warpNonceRef.current += 1;
      setWarpAnchor({
        hash: v.anchoredCommit.hash,
        nonce: warpNonceRef.current,
        expand: v.anchorExpand,
      });
    } else {
      setWarpAnchor(null);
    }
    setControlNotice(null);
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
    dismissContextMenus();
    const target = viewBack[viewBack.length - 1];
    setViewFwd((f) => [viewRef.current, ...f].slice(0, VIEW_HISTORY_MAX));
    setViewBack((b) => b.slice(0, -1));
    applyView(target);
  }, [viewBack, applyView]);

  const goForward = useCallback(() => {
    if (viewFwd.length === 0) return;
    dismissContextMenus();
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
      branchHydrationEpochRef.current += 1;
      setSelectedRepoPath(c.repoPath);
      setSelectedBranches("all");
      setSelectedAuthors("all");
      setWindowDays((cur) => windowCovering(cur, c.timestamp));
      setSearchOpen(false);
      setSearchCount(null);
      setAnchoredCommit(c);
      setControlNotice(null);
      warpNonceRef.current += 1;
      setWarpAnchor({
        hash: c.hash,
        nonce: warpNonceRef.current,
        expand: false,
      });
    },
    [pushView],
  );

  const applyControlDelivery = useCallback((delivery: ControlDelivery) => {
    const source = delivery.source.name || "an external tool";
    if (delivery.status === "error") {
      setControlNotice({
        tone: "error",
        source,
        detail: delivery.error.message,
      });
      return;
    }

    if (delivery.target.kind === "commit") {
      const { commit, presentation } = delivery.target;
      const current = viewRef.current;
      const alreadyLanded =
        current.fileHistory == null &&
        current.anchoredCommit?.hash === commit.hash &&
        current.repoPath != null &&
        sameCanonicalPath(current.repoPath, commit.repoPath);
      if (!alreadyLanded) pushView();
      setFileHistory(null);
      if (!sameCanonicalPath(commit.repoPath, selectedRepoPathRef.current ?? "")) {
        pendingBranchesRef.current = "all";
      }
      branchHydrationEpochRef.current += 1;
      setSelectedRepoPath(commit.repoPath);
      setSelectedBranches("all");
      setSelectedAuthors("all");
      setWindowDays((current) => windowCovering(current, commit.timestamp));
      setSearchOpen(false);
      setSearchCount(null);
      setAnchoredCommit(commit);
      setControlNotice({
        tone: "ready",
        source,
        detail: `commit ${commit.shortHash}`,
      });
      warpNonceRef.current += 1;
      setWarpAnchor({
        hash: commit.hash,
        nonce: warpNonceRef.current,
        expand: presentation.expand,
      });
      return;
    }

    const { repoPath, filePath } = delivery.target;
    const current = viewRef.current;
    const alreadyOpen =
      current.fileHistory != null &&
      sameCanonicalPath(current.fileHistory.repoPath, repoPath) &&
      current.fileHistory.filePath === filePath;
    if (alreadyOpen) {
      setRefsNonce((nonce) => nonce + 1);
    } else {
      pushView();
      setSearchOpen(false);
      setSearchCount(null);
      setWarpAnchor(null);
      setAnchoredCommit(null);
      setSelectedRepoPath(repoPath);
      setFileHistoryCoverage(null);
      setFileHistoryError(false);
      setFileHistory({ repoPath, filePath });
      setRefreshNonce((nonce) => nonce + 1);
    }
    setControlNotice({
      tone: "ready",
      source,
      detail: `history for ${filePath}`,
    });
  }, [pushView]);

  useControlDelivery(applyControlDelivery);

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
    if (current == null || !sameCanonicalPath(current, repo)) return; // repo left — drop
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

  const togglePanelPinned = useCallback(async () => {
    const next = !pinned;
    try {
      // Refuse to flip the glyph if persistence fails, so the current UI and
      // the next launch cannot disagree.
      await invoke("set_panel_pinned", { pinned: next });
    } catch (error) {
      // eslint-disable-next-line no-console
      console.error("[gitwink] set_panel_pinned failed", error);
      return;
    }
    void broadcastSettings({
      ...getCurrentSettings(),
      panelPinned: next,
    });
  }, [pinned]);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchCount(null);
    setSearchActiveDescendant(null);
    setResolvedSearchQuery(null);
    setResolvedSearchFilterKey(null);
    setResolvedSearchEpoch(null);
  }, []);

  const acceptResolvedSearchSnapshot = useCallback(
    (query: string | null, filterKey: string, resolutionEpoch: number) => {
      setResolvedSearchQuery(query ?? "");
      setResolvedSearchFilterKey(filterKey);
      setResolvedSearchEpoch(resolutionEpoch);
    },
    [],
  );

  const changeSearchInput = useCallback((next: string) => {
    // Editing after Back is a new branch of navigation history, just like a
    // time/author/branch refinement. A stale Forward target must not survive.
    setViewFwd([]);
    setSearchCount(null);
    setSearchActiveDescendant(null);
    setSearchInput(next);
  }, []);

  const activateSearchSelection = useCallback(() => {
    if (!searchResolved) {
      // Enter during the debounce window means "run what I can see", never
      // "open a row from the previous query". Flush the visible text and
      // require a second Enter once its results have resolved.
      setSearchQuery(searchInput);
      return;
    }
    searchControlRef.current?.activateSelected();
  }, [searchInput, searchResolved]);

  // Every visible way into search funnels here — header button, empty-
  // state action, `/` hotkey — so they all focus the input the same way.
  const openSearch = useCallback(() => {
    dismissContextMenus();
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
      setRepoCommitsCoverage(null);
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
      setRepoCommitsCoverage({
        truncated: false,
        incomplete: false,
        limit: 0,
        shallow: false,
      });
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
        if (
          sameView &&
          canReuseRepoCommitsSnapshot(commitsRefsFpRef.current, fp)
        ) {
          return;
        }
        const result = await repoCommits(
          selectedRepoPath,
          branchParam,
          toWindowParam(windowDays),
        );
        if (!cancelled) {
          // A partial read or shallow boundary can recover without any ref
          // tip moving (objects arrive, or the clone is deepened). Never let
          // that provisional result suppress the next explicit re-summon.
          commitsRefsFpRef.current = reusableRepoCommitsFingerprint(fp, result);
          setCommits(result.rows);
          setRepoCommitsCoverage({
            truncated: result.truncated,
            incomplete: result.incomplete,
            limit: result.limit,
            shallow: result.shallow,
          });
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
    const operationId = `${Date.now().toString(36)}-${(
      ++fileHistoryOperationRef.current
    ).toString(36)}`;
    setCommits(null);
    setFileHistoryCoverage(null);
    setFileHistoryError(false);
    (async () => {
      try {
        const result = await fetchFileHistory(
          repoPath,
          filePath,
          operationId,
          FILE_HISTORY_FLOW_ID,
        );
        // Backend cancellation is a normal null outcome. It must not flash a
        // false "Couldn't open" error while the replacement scope is loading.
        if (!cancelled && result) {
          const { rows, ...coverage } = result;
          setCommits(rows);
          setFileHistoryCoverage(coverage);
          setFileHistoryError(false);
        }
      } catch {
        if (!cancelled) {
          setCommits([]);
          setRepoCommitsCoverage(null);
          setFileHistoryCoverage(null);
          setFileHistoryError(true);
        }
      }
    })();
    return () => {
      cancelled = true;
      void cancelFileHistory(operationId, FILE_HISTORY_FLOW_ID).catch(() => {});
    };
  }, [singleMode, selectedRepoPath, fileHistory, refsNonce]);

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
      // UpdateModal is the sole owner of its close policy. In particular,
      // Escape must stay inert while an install is in progress; duplicating
      // that decision here let a body-targeted key event bypass the modal's
      // `onClose={undefined}` guard.
      if (updateModal) return;
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
        setAnchoredCommit(null);
        setControlNotice(null);
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

  const singleRepoCommitsWithAnchor = useMemo(
    () =>
      mergeAnchoredCommit(
        commits,
        anchoredCommit,
        selectedRepoPath,
        fileHistory != null,
      ),
    [commits, anchoredCommit, selectedRepoPath, fileHistory],
  );

  // A ref-stable re-summon deliberately skips the expensive Git walk, but a
  // rolling window still moves. Prune the already-loaded rows locally at the
  // exact next expiry second; refs cannot add rows while their fingerprint is
  // unchanged, so this is both cheaper and more accurate than re-walking.
  useEffect(() => {
    if (
      !singleMode ||
      fileHistory != null ||
      windowDays === "all" ||
      singleRepoCommitsWithAnchor == null
    ) {
      return;
    }
    const now = Math.floor(Date.now() / 1000);
    setSingleRepoWindowNow(now);
    const nextExpiry = nextRollingWindowExpiry(
      singleRepoCommitsWithAnchor,
      windowDays,
      now,
    );
    if (nextExpiry == null) return;
    const delay = Math.max(
      100,
      Math.min((nextExpiry - now) * 1000 + 50, 2_147_000_000),
    );
    const timer = window.setTimeout(
      () => setSingleRepoWindowNow(Math.floor(Date.now() / 1000)),
      delay,
    );
    return () => window.clearTimeout(timer);
  }, [
    singleMode,
    fileHistory,
    windowDays,
    singleRepoCommitsWithAnchor,
    refreshNonce,
    singleRepoWindowNow,
  ]);

  const timeScopedSingleRepoCommits = useMemo(() => {
    if (!singleRepoCommitsWithAnchor || fileHistory != null) {
      return singleRepoCommitsWithAnchor;
    }
    return filterRollingWindow(
      singleRepoCommitsWithAnchor,
      windowDays,
      singleRepoWindowNow,
    );
  }, [
    singleRepoCommitsWithAnchor,
    fileHistory,
    windowDays,
    singleRepoWindowNow,
  ]);

  // Single-repo mode tallies authors from its (bounded) visible commits;
  // all-repos mode uses the backend facet (`authorsAll`).
  const authorsSingle: AuthorTally[] = useMemo(() => {
    const m = new Map<string, { count: number; lastActivity: number }>();
    for (const c of timeScopedSingleRepoCommits ?? []) {
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
  }, [timeScopedSingleRepoCommits]);
  const authors = singleMode ? authorsSingle : authorsAll;
  const branchCatalogIsCurrent =
    branchesRepoPathRef.current === selectedRepoPath;
  const visibleBranches = branchCatalogIsCurrent ? branches : [];
  const visibleBranchesLoading =
    singleMode && (!branchCatalogIsCurrent || branchesLoading);

  // Single-repo mode only — the all-repos timeline filters server-side.
  // The repo filter doesn't apply here (you're inside one repo already);
  // just narrow the loaded commits by the author selection.
  const filteredCommits = useMemo(() => {
    if (!timeScopedSingleRepoCommits) return null;
    // File history shows every commit that touched the file — the author
    // lens is dimmed/inert in that mode, so don't filter by it.
    if (fileHistory) return timeScopedSingleRepoCommits;
    if (selectedAuthors === "all") return timeScopedSingleRepoCommits;
    const set = new Set(selectedAuthors);
    return timeScopedSingleRepoCommits.filter((c) => set.has(c.author));
  }, [timeScopedSingleRepoCommits, selectedAuthors, fileHistory]);

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
  // count can't drift from reality. null means the explicit all-repos state;
  // an unresolved narrow scope gets its own loading label instead.
  const searchScopeLabel = useMemo<string | null>(() => {
    if (searchRepoIds == null) return null;
    if (searchRepoIds.length === 0) {
      return i18n._(panelMessages.selectedRepoScopeLoading);
    }
    if (searchRepoIds.length === 1) {
      const id = searchRepoIds[0];
      return (
        allRepos.find((r) => r.id === id)?.name ??
        translate(i18n, panelMessages.repoCount, { count: 1 })
      );
    }
    return translate(i18n, panelMessages.repoCount, {
      count: searchRepoIds.length,
    });
  }, [searchRepoIds, allRepos, i18n, i18n.locale]);

  // Only offer "Search all repos" when widening would genuinely broaden the
  // scope — i.e. more usable repos exist than the search currently covers.
  const canWidenSearch =
    searchRepoIds != null &&
    searchRepoIds.length > 0 &&
    usableRepoCount > searchRepoIds.length;

  // Widen a scoped search to every repo and let it re-run: a real navigation
  // (records the scoped view so back returns to it), dropping single-repo mode
  // and the RepoChip narrowing while keeping the query/search open.
  const widenSearchScope = useCallback(() => {
    pushView();
    setFileHistory(null);
    setWarpAnchor(null);
    setAnchoredCommit(null);
    setControlNotice(null);
    setSelectedRepoPath(null);
    setSelectedRepoPaths("all");
  }, [pushView]);

  // Time/author changes refine the current view in place — no new history
  // entry, but they diverge from any forward history, so drop it.
  const changeWindowDays = useCallback((v: WindowDays) => {
    setViewFwd([]);
    setWarpAnchor(null);
    setAnchoredCommit(null);
    setControlNotice(null);
    setWindowDays(v);
  }, [i18n]);
  const changeAuthors = useCallback((v: string[] | "all") => {
    setViewFwd([]);
    setWarpAnchor(null);
    setAnchoredCommit(null);
    setControlNotice(null);
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
      setAnchoredCommit(null);
      setControlNotice(null);
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
      setAnchoredCommit(null);
      setControlNotice(null);
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
    setAnchoredCommit(null);
    setControlNotice(null);
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
      setAnchoredCommit(null);
      setControlNotice(null);
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

  return (
    <main className={"panel" + (singleMode ? " single-mode" : "")}>
      <header
        className="panel-header"
        onPointerDown={startDrag}
        onContextMenu={(e) => {
          const target = e.target as HTMLElement;
          // Interactive controls own their pointer behaviour. Empty chrome,
          // drag space and passive status badges expose Settings.
          if (target.closest("button, [role=button], .chip-wrap, .context-menu")) {
            return;
          }
          e.preventDefault();
          setHeaderCtxMenu({
            x: e.clientX,
            y: e.clientY,
            items: [
              {
                label: i18n._(panelMessages.settingsMenu),
                onClick: () => void invoke("open_settings_window"),
              },
            ],
          });
        }}
      >
        <PanelChromeRow
          updateVersion={updateVersion}
          onOpenUpdate={openUpdateModal}
          canGoBack={viewBack.length > 0}
          canGoForward={viewFwd.length > 0}
          onBack={goBack}
          onForward={goForward}
          scanning={scanning}
          updateCheckNotice={updateCheckNotice}
          upstream={upstream}
          showRepoSync={singleMode && !fileHistory && !searchOpen}
          fetchNow={fetchNow}
          onFetchNow={() => void runFetchNow()}
          searchOpen={searchOpen}
          onToggleSearch={() => (searchOpen ? closeSearch() : openSearch())}
          pinned={pinned}
          onTogglePinned={() => void togglePanelPinned()}
          onDismiss={() => void dismissPanel()}
        />

        <PanelFilterRow
          searchOpen={searchOpen}
          searchInput={searchInput}
          searching={searching}
          searchResolved={searchResolved}
          searchCount={searchCount}
          searchFocusNonce={searchFocusNonce}
          searchActiveDescendant={searchActiveDescendant}
          searchScopeLabel={searchScopeLabel}
          historyDemanded={historyDemanded}
          historyIndexError={historyIndexError}
          historyIndexStatus={historyIndexStatus}
          searchControlRef={searchControlRef}
          onSearchChange={changeSearchInput}
          onCloseSearch={closeSearch}
          onActivateSearch={activateSearchSelection}
          openChip={openChip}
          onOpenChip={setOpenChip}
          repos={allRepos}
          repoCounts={repoCounts}
          repoCountsLoading={repoCountsLoading}
          pinnedRepos={pinnedRepos}
          selectedRepoPath={selectedRepoPath}
          selectedRepoPaths={selectedRepoPaths}
          onSelectRepo={changeRepoPath}
          onSelectRepos={changeRepoPaths}
          onSelectAllRepos={resetRepoScope}
          onToggleRepoPin={togglePin}
          onExcludeRepo={handleExcludeRepo}
          fileHistory={fileHistory}
          fileHistoryCoverage={fileHistoryCoverage}
          fileHistoryError={fileHistoryError}
          fileHistoryMatchCount={commits?.length ?? 0}
          onExitFileHistory={exitFileHistory}
          singleMode={singleMode}
          branches={visibleBranches}
          branchesLoading={visibleBranchesLoading}
          selectedBranches={selectedBranches}
          onBranchesChange={handleBranchChange}
          windowDays={windowDays}
          onWindowDaysChange={changeWindowDays}
          authors={authors}
          authorsLoading={authorsLoading}
          selectedAuthors={selectedAuthors}
          onAuthorsChange={changeAuthors}
        />
      </header>
      <PanelNotices
        showFacetError={!singleMode && !searchOpen && facetsError}
        onRetryFacets={retryFacets}
        historyDemanded={historyDemanded}
        historyIndexError={historyIndexError}
        historyIndexStatus={historyIndexStatus}
        onRetryHistoryIndex={() =>
          setHistoryIndexRetryNonce((nonce) => nonce + 1)
        }
        showAutoFetchNotice={showAutoFetchNotice}
        onOpenSettings={() => void invoke("open_settings_window")}
        onDismissAutoFetchNotice={dismissAutoFetchNotice}
        controlNotice={controlNotice}
        onDismissControlNotice={() => setControlNotice(null)}
      />
      <section className="panel-body">
        {!firstRunTipSeen && !searching && !fileHistory && allRepos.length > 0 && (
          <FirstRunTip onDismiss={dismissFirstRunTip} />
        )}
        {allRepos.length === 0 && !singleMode ? (
          <EmptyDropPanel
            scanning={scanning}
            addError={addError}
            onBrowse={() => void browseForRepo()}
          />
        ) : (
          <>
            {searching && (
              // Active search: the timeline body IS the result list — the
              // same windowed machinery with the query as one more filter
              // (time/author bypassed, repo scope respected). Enter / ↗
              // warps into the commit's single-repo history.
              <div
                className={
                  "search-results-host" +
                  (searchResolved ? "" : " search-results-pending")
                }
                aria-busy={!searchResolved}
              >
                {searchQuery.trim().length > 0 && (
                  <TimelineWindowed
                    key="search-results"
                    repoIds={searchRepoIds}
                    authors={null}
                    windowDays={null}
                    refreshNonce={refreshNonce + historyRefreshNonce}
                    query={searchQuery}
                    skipRefill
                    searchMode
                    onWarp={performWarp}
                    searchControlRef={searchControlRef}
                    onResultCount={setSearchCount}
                    resultsListId={SEARCH_RESULTS_LIST_ID}
                    onActiveDescendant={setSearchActiveDescendant}
                    onResolvedQuery={acceptResolvedSearchSnapshot}
                    onResolutionPending={clearResolvedSearchSnapshot}
                    onSelectRepo={changeRepoPath}
                    searchScopeLabel={searchScopeLabel}
                    onWidenSearch={canWidenSearch ? widenSearchScope : undefined}
                    expansionControlRef={expansionControlRef}
                    inactive={!searchResolved}
                  />
                )}
                {!searchResolved && (
                  <div className="search-resolving-cover" role="status">
                    {i18n._(panelMessages.searching)}
                  </div>
                )}
              </div>
            )}
            {!searching && singleMode && (
              <SingleRepoView
                filteredCommits={filteredCommits}
                allCommits={singleRepoCommitsWithAnchor}
                commitsError={commitsError}
                repoCoverage={repoCommitsCoverage}
                fileHistory={fileHistory}
                fileHistoryCoverage={fileHistoryCoverage}
                fileHistoryError={fileHistoryError}
                branches={branches}
                selectedRepoPath={selectedRepoPath}
                selectedAuthors={selectedAuthors}
                selectedBranches={selectedBranches}
                windowDays={windowDays}
                anchor={warpAnchor}
                expansionControlRef={expansionControlRef}
                onBackToAllRepos={resetRepoScope}
                onRetryFileHistory={() => setRefsNonce((nonce) => nonce + 1)}
                onExitFileHistory={exitFileHistory}
                onShowAllTime={() => changeWindowDays("all")}
                onClearAuthors={() => changeAuthors("all")}
                onAllBranches={() => handleBranchChange("all")}
                onOpenSearch={openSearch}
              />
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
                refreshNonce={refreshNonce + historyRefreshNonce}
                // This instance deliberately stays mounted to preserve the
                // browse viewport. While another view owns the body it stays
                // fully suspended: no cache re-query and no hidden fleet Git
                // scan competing with search/direct/file-history work.
                skipRefill={
                  historyDemanded || searching || singleMode || fileHistory != null
                }
                onSelectRepo={changeRepoPath}
                onShowAllTime={() => changeWindowDays("all")}
                onClearAuthors={() => changeAuthors("all")}
                onOpenSearch={openSearch}
                onSnapshotGeneration={acceptBrowseSnapshot}
                expansionControlRef={expansionControlRef}
                inactive={searching || singleMode}
                suspended={searching || singleMode}
              />
            </div>
          </>
        )}
        {allRepos.length > 0 && (
          <PanelFooter
            addError={addError}
            onAddRepo={() => void browseForRepo()}
          />
        )}
      </section>
      {updateModal && (
        <UpdateModal
          state={updateModal}
          onClose={() => setUpdateModal(null)}
        />
      )}
      {headerCtxMenu && !updateModal && (
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

export default App;
