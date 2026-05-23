import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { AuthorsChip } from "./components/AuthorsChip";
import { BranchChip } from "./components/BranchChip";
import { ContextMenu, type MenuItem } from "./components/ContextMenu";
import { RepoChip } from "./components/RepoChip";
import { Timeline } from "./components/Timeline";
import { TimelineWindowed } from "./components/TimelineWindowed";
import { TimeRangeChip } from "./components/TimeRangeChip";
import {
  currentUpstreamStatus,
  dismissPanel,
  explicitAddRepo,
  getBranchSelection,
  getPinnedRepos,
  getScanState,
  hideRepo,
  listBranches,
  listFilterFacets,
  listRepos,
  onOrchestratorProgress,
  onPanelShown,
  onRepoDiscovered,
  onUpdateNone,
  onUpdateShowModal,
  repoCommits,
  setBranchSelection as saveBranchSelection,
  setPanelSticky,
  setPinnedRepos as savePinnedRepos,
  updateGetState,
} from "./lib/ipc";
import {
  broadcastSettings,
  getCurrentSettings,
  usePanelPinned,
} from "./lib/settings";
import { UpdateModal } from "./components/UpdateModal";
import type {
  AuthorTally,
  BranchInfo,
  CommitSummary,
  Repo,
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
 * to the BranchChip in single-repo mode. Reads from local refs only —
 * gitwink never calls git fetch. Tooltip explains the last-fetch caveat
 * so users don't expect live remote state. */
function UpstreamBadge({ status }: UpstreamBadgeProps) {
  const synced = status.ahead === 0 && status.behind === 0;
  const aheadStr = status.ahead.toString() + (status.aheadCapped ? "+" : "");
  const behindStr = status.behind.toString() + (status.behindCapped ? "+" : "");
  const fetchHint = status.lastFetchUnix
    ? `Last fetch: ${formatFetchAge(status.lastFetchUnix)}`
    : "No fetch recorded yet";
  const title = synced
    ? `${status.localBranch} is in sync with ${status.upstream}.\n${fetchHint}. gitwink never calls git fetch itself.`
    : `${status.localBranch} vs ${status.upstream}: ${status.ahead} ahead, ${status.behind} behind.\n${fetchHint}. gitwink never calls git fetch itself.`;

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

function App() {
  const [scanning, setScanning] = useState(false);
  const [commits, setCommits] = useState<CommitSummary[] | null>(null);
  const [allRepos, setAllRepos] = useState<Repo[]>([]);
  const [discoveredCount, setDiscoveredCount] = useState<number | null>(null);
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
  // Transient "you're up to date" line after a manual check found nothing.
  const [upToDate, setUpToDate] = useState(false);

  const singleMode = selectedRepoPath != null;

  // ----- bootstrap -----
  useEffect(() => {
    let mounted = true;
    let unProgress: UnlistenFn | undefined;
    let unDiscovered: UnlistenFn | undefined;
    let unStatus: UnlistenFn | undefined;
    let unShown: UnlistenFn | undefined;
    let unUpdateModal: UnlistenFn | undefined;
    let unUpdateNone: UnlistenFn | undefined;

    (async () => {
      try {
        const repos = await listRepos();
        if (mounted) {
          setAllRepos(repos);
          setDiscoveredCount(repos.length);
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
        setDiscoveredCount(p.reposFound);
        setScanning(p.state === "scanning");
        // Scan finished — refresh the repo list so newly-found repos carry
        // their real ids (needed to filter the windowed timeline by repo)
        // and bump the nonce so the timeline + author facet re-pull.
        if (p.state === "complete") {
          void listRepos()
            .then((repos) => {
              if (mounted) {
                setAllRepos(repos);
                setDiscoveredCount(repos.length);
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
        if (mounted) setRefreshNonce((n) => n + 1);
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

      // Per-repo discovery: merge into allRepos so the repo chip
      // dropdown lights up as repos are validated. Refresh cached
      // commits opportunistically so the timeline picks up rows from
      // the newly-discovered repo without a manual reload.
      unDiscovered = await onRepoDiscovered((p) => {
        if (!mounted) return;
        setAllRepos((prev) => {
          if (prev.some((r) => r.path === p.path)) return prev;
          // Orchestrator only emits for validated repos, so status='active'
          // is correct on insert. The id is unknown until the scan-complete
          // listRepos() refresh backfills it — harmless, the windowed
          // timeline's repo filter ignores id 0.
          const next = [
            ...prev,
            { id: 0, path: p.path, name: p.name, status: "active" as const },
          ];
          // Keep stable display order to avoid jitter in the chip dropdown.
          next.sort((a, b) => a.name.localeCompare(b.name));
          return next;
        });
        setDiscoveredCount((prev) => (prev ?? 0) + 1);
      });

      // Repo status transitions (active ↔ missing ↔ removed) — backend
      // emits one event per row that changed. Patch allRepos in place
      // so the RepoChip row greys out / restores / drops without a
      // full reload.
      const { listen } = await import("@tauri-apps/api/event");
      unStatus = await listen<{ canonicalPath: string; status: string }>(
        "timeline://repo-status",
        (e) => {
          if (!mounted) return;
          const { canonicalPath, status } = e.payload;
          if (status === "removed") {
            setAllRepos((prev) => prev.filter((r) => r.path !== canonicalPath));
            setDiscoveredCount((prev) =>
              prev != null ? Math.max(0, prev - 1) : prev,
            );
            return;
          }
          if (status === "active" || status === "missing") {
            setAllRepos((prev) =>
              prev.map((r) =>
                r.path === canonicalPath
                  ? { ...r, status: status as "active" | "missing" }
                  : r,
              ),
            );
          }
        },
      );

    })();

    return () => {
      mounted = false;
      unProgress?.();
      unDiscovered?.();
      unStatus?.();
      unShown?.();
      unUpdateModal?.();
      unUpdateNone?.();
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

  // ----- single-repo mode: branch list + saved selection -----
  // Both depend ONLY on the repo — the branch set is window-independent,
  // so refetching when the time window changes would be wasteful. On
  // repo change we reset selectedBranches to "all" up front so the
  // commits effect never fires with a stale per-repo selection, then
  // restore this repo's saved selection if it has one. Absence of a
  // saved selection ⇒ "all", the first-entry default.
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) {
      setBranches([]);
      setSelectedBranches("all");
      return;
    }
    setSelectedBranches("all");
    let cancelled = false;
    (async () => {
      try {
        const saved = await getBranchSelection(selectedRepoPath);
        if (!cancelled && saved.length > 0) setSelectedBranches(saved);
      } catch {}
      try {
        const bs = await listBranches(selectedRepoPath);
        if (!cancelled) setBranches(bs);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, selectedRepoPath]);

  // Persist the BranchChip selection per repo so it survives across
  // sessions. "all" is stored as an empty list (absence ⇒ "all"), so the
  // first-entry default and an explicit "all" pick collapse to the same
  // thing.
  const handleBranchChange = useCallback(
    (sel: string[] | "all") => {
      setSelectedBranches(sel);
      if (selectedRepoPath) {
        void saveBranchSelection(selectedRepoPath, sel === "all" ? [] : sel);
      }
    },
    [selectedRepoPath],
  );

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
  }, [selectedRepoPath, singleMode, selectedBranches]);

  // ----- single-repo mode: commits (depends on repo, branches, window) -----
  // All three dimensions of the filter must be in the dep list — otherwise
  // changing windowDays or selectedRepoPath while a branch filter is
  // active clobbers the filter (the BranchChip shows "feature" but
  // Timeline silently flips to all-branches). Empty explicit selection
  // returns [] immediately without hitting the backend so "No branches"
  // really means no rows.
  useEffect(() => {
    if (!singleMode || !selectedRepoPath) return;
    if (selectedBranches !== "all" && selectedBranches.length === 0) {
      setCommits([]);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const branchParam =
          selectedBranches === "all" ? null : selectedBranches;
        const cs = await repoCommits(
          selectedRepoPath,
          branchParam,
          toWindowParam(windowDays),
        );
        if (!cancelled) setCommits(cs);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [singleMode, selectedRepoPath, selectedBranches, windowDays, refreshNonce]);

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
      setAllRepos((prev) => {
        if (prev.some((r) => r.path === repo.path)) return prev;
        const next = [
          ...prev,
          { id: 0, path: repo.path, name: repo.name, status: "active" as const },
        ];
        next.sort((a, b) => a.name.localeCompare(b.name));
        return next;
      });
      // Refresh so the new repo carries its real id — the windowed timeline
      // filters by id, not path.
      void listRepos()
        .then((repos) => setAllRepos(repos))
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

  // ----- ESC layer cascade: chip → expansion (in Timeline) → single-repo → hide panel -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      // The update modal is the top-most layer — close it first.
      if (updateModal) {
        setUpdateModal(null);
        e.preventDefault();
        return;
      }
      if (openChip != null) return; // dropdown handles its own Esc
      // Timeline's Esc handler stopImmediatePropagation()s if an
      // expansion is open, so by the time we get here neither a
      // dropdown nor an expansion needs Esc.
      if (singleMode) {
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
  }, [openChip, singleMode, updateModal]);

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
    if (selectedAuthors === "all") return commits;
    const set = new Set(selectedAuthors);
    return commits.filter((c) => set.has(c.author));
  }, [commits, selectedAuthors]);

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

  function togglePin(path: string) {
    setPinnedRepos((prev) => {
      const next = prev.includes(path)
        ? prev.filter((p) => p !== path)
        : [...prev, path];
      void savePinnedRepos(next);
      return next;
    });
  }

  const repoCount = discoveredCount ?? allRepos.length;

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
          if (target.closest(".chip-wrap, .panel-close")) return;
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
        <img
          src="/icon.png"
          alt="gitwink"
          title="gitwink"
          className="panel-header-icon"
          draggable={false}
        />
        <div className="header-chips">
          <RepoChip
            open={openChip === "repo"}
            onToggle={() => setOpenChip(openChip === "repo" ? null : "repo")}
            onClose={() => setOpenChip(null)}
            repos={allRepos}
            repoCounts={repoCounts}
            pinned={pinnedRepos}
            selectedPath={selectedRepoPath}
            selectedPaths={selectedRepoPaths}
            onSelect={setSelectedRepoPath}
            onSelectMulti={setSelectedRepoPaths}
            onTogglePin={togglePin}
            onHide={(path) => {
              // Optimistic: drop from local list immediately; backend
              // will tombstone so it stays gone across restarts.
              setAllRepos((prev) => prev.filter((r) => r.path !== path));
              setDiscoveredCount((prev) =>
                prev != null ? Math.max(0, prev - 1) : prev,
              );
              void hideRepo(path).catch(() => {
                // If the backend rejects (race / already gone), fall
                // back to re-fetching the list so UI matches truth.
                void listRepos().then((r) => setAllRepos(r));
              });
            }}
            totalRepoCount={repoCount}
          />
          {singleMode && (
            <BranchChip
              open={openChip === "branch"}
              onToggle={() =>
                setOpenChip(openChip === "branch" ? null : "branch")
              }
              onClose={() => setOpenChip(null)}
              branches={branches}
              selected={selectedBranches}
              onChange={handleBranchChange}
            />
          )}
          {singleMode && upstream && (
            <UpstreamBadge status={upstream} />
          )}
          <TimeRangeChip
            open={openChip === "time"}
            onToggle={() => setOpenChip(openChip === "time" ? null : "time")}
            onClose={() => setOpenChip(null)}
            value={windowDays}
            onChange={setWindowDays}
          />
          <AuthorsChip
            open={openChip === "authors"}
            onToggle={() =>
              setOpenChip(openChip === "authors" ? null : "authors")
            }
            onClose={() => setOpenChip(null)}
            authors={authors}
            selected={selectedAuthors}
            onChange={setSelectedAuthors}
          />
        </div>
        <div className="panel-drag-handle" />
        {scanning && <span className="panel-status">Scanning…</span>}
        {upToDate && !scanning && (
          <span className="panel-status">✓ Up to date</span>
        )}
        <button
          type="button"
          className={"panel-pin" + (pinned ? " pinned" : "")}
          onClick={async () => {
            const next = !pinned;
            await invoke("set_panel_pinned", { pinned: next });
            // Mirror to lib/settings + emit to every window so the pin
            // glyph flips immediately (rather than waiting for the next
            // get_settings round-trip).
            broadcastSettings({ ...getCurrentSettings(), panelPinned: next });
          }}
          title={
            pinned
              ? "Unpin — return to glance mode (auto-hides on blur). The taskbar entry disappears on next launch."
              : "Pin — keep open while clicking elsewhere. The taskbar entry appears on next launch."
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
      <section className="panel-body">
        {allRepos.length === 0 && !singleMode ? (
          <EmptyDropPanel
            scanning={scanning}
            addError={addError}
            onBrowse={() => void handleAddRepo()}
          />
        ) : singleMode ? (
          filteredCommits == null ? (
            <p className="panel-empty">Loading commits…</p>
          ) : (
            <Timeline
              key={`single:${selectedRepoPath}`}
              commits={filteredCommits}
              branches={branches}
            />
          )
        ) : (
          <TimelineWindowed
            repoIds={repoIds}
            authors={selectedAuthors === "all" ? null : selectedAuthors}
            windowDays={toWindowParam(windowDays)}
            refreshNonce={refreshNonce}
            onSelectRepo={setSelectedRepoPath}
          />
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
