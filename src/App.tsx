import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { AuthorsChip } from "./components/AuthorsChip";
import { BranchChip } from "./components/BranchChip";
import { RepoChip } from "./components/RepoChip";
import { Timeline } from "./components/Timeline";
import { TimeRangeChip } from "./components/TimeRangeChip";
import {
  discoverRepos,
  dismissPanel,
  getPinnedRepos,
  listBranches,
  listRecentCommitsCached,
  listRepos,
  onScanComplete,
  onScanProgress,
  onTimelineRepoFill,
  recentCommits,
  repoCommits,
  setPinnedRepos as savePinnedRepos,
} from "./lib/ipc";
import type {
  AuthorTally,
  BranchInfo,
  CommitSummary,
  Repo,
  WindowDays,
} from "./types";
import "./styles.css";

const TIMELINE_MAX = 50;

function startDrag(e: React.PointerEvent<HTMLElement>) {
  if (e.button !== 0) return;
  const target = e.target as HTMLElement | null;
  // Don't start a drag if the press landed on a clickable control or
  // inside an open chip dropdown (incl. its scrollbars / inputs / pin
  // buttons). The user is interacting with the dropdown, not the window.
  if (target?.closest("button, input, .chip-dropdown, [data-no-drag]")) return;
  void getCurrentWindow().startDragging();
}

function mergeCommits(
  prev: CommitSummary[],
  incoming: CommitSummary[],
): CommitSummary[] {
  const map = new Map<string, CommitSummary>();
  for (const c of prev) map.set(`${c.repoPath}:${c.hash}`, c);
  for (const c of incoming) map.set(`${c.repoPath}:${c.hash}`, c);
  return Array.from(map.values())
    .sort((a, b) => b.timestamp - a.timestamp)
    .slice(0, TIMELINE_MAX);
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

  const [windowDays, setWindowDays] = useState<WindowDays>(7);
  const [selectedRepoPath, setSelectedRepoPath] = useState<string | null>(null);
  const [selectedAuthors, setSelectedAuthors] = useState<string[] | "all">(
    "all",
  );

  // Single-repo mode state.
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [selectedBranches, setSelectedBranches] = useState<string[] | "all">(
    "all",
  );

  // Fresh-commit tracking — populated by file-watcher pushes, cleared on
  // panel blur (i.e. "the user has seen this, mark as read on close").
  const [freshHashes, setFreshHashes] = useState<Set<string>>(() => new Set());

  const [openChip, setOpenChip] = useState<
    "repo" | "time" | "authors" | "branch" | null
  >(null);

  const singleMode = selectedRepoPath != null;

  // Mirror selectedRepoPath into a ref so the file-watcher listener — set up
  // once in the bootstrap effect — can read the *current* value instead of
  // the null it captured at mount.
  const selectedRepoPathRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRepoPathRef.current = selectedRepoPath;
  }, [selectedRepoPath]);

  // ----- bootstrap (All-repos timeline) -----
  useEffect(() => {
    let mounted = true;
    let unP: UnlistenFn | undefined;
    let unC: UnlistenFn | undefined;
    let unF: UnlistenFn | undefined;

    (async () => {
      try {
        const cached = await listRecentCommitsCached(toWindowParam(windowDays));
        if (mounted) setCommits(cached);
      } catch {}

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

      unP = await onScanProgress((p) => {
        if (mounted) setDiscoveredCount(p.found);
      });
      unC = await onScanComplete(async (p) => {
        if (!mounted) return;
        setDiscoveredCount(p.count);
        setScanning(false);
        try {
          const repos = await listRepos();
          if (mounted) setAllRepos(repos);
        } catch {}
      });
      unF = await onTimelineRepoFill((p) => {
        if (!mounted) return;
        // Only merge into the All-repos timeline; ignore while in single mode.
        // Read the *current* selectedRepoPath via ref — the value captured in
        // this closure at mount time is permanently null.
        setCommits((prev) =>
          selectedRepoPathRef.current
            ? prev
            : mergeCommits(prev ?? [], p.commits),
        );
        if (p.fresh && p.commits.length > 0) {
          setFreshHashes((prev) => {
            const next = new Set(prev);
            for (const c of p.commits) next.add(`${c.repoPath}:${c.hash}`);
            return next;
          });
        }
      });

      setScanning(true);
      void discoverRepos().catch(() => {
        if (mounted) setScanning(false);
      });
    })();

    return () => {
      mounted = false;
      unP?.();
      unC?.();
      unF?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ----- refresh All-repos commits when time window changes -----
  useEffect(() => {
    if (singleMode) return;
    let cancelled = false;
    (async () => {
      try {
        const cached = await listRecentCommitsCached(toWindowParam(windowDays));
        if (!cancelled) setCommits(cached);
      } catch {}
      try {
        const fresh = await recentCommits(toWindowParam(windowDays));
        if (!cancelled) setCommits(fresh);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
  }, [windowDays, singleMode]);

  // ----- single-repo mode: load branches + commits -----
  useEffect(() => {
    if (!singleMode) {
      setBranches([]);
      setSelectedBranches("all");
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const bs = await listBranches(selectedRepoPath!);
        if (!cancelled) setBranches(bs);
      } catch {}
      try {
        const cs = await repoCommits(
          selectedRepoPath!,
          null,
          toWindowParam(windowDays),
        );
        if (!cancelled) setCommits(cs);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedRepoPath, windowDays]);

  // ----- single-repo mode: refresh commits when branches selection changes -----
  useEffect(() => {
    if (!singleMode) return;
    let cancelled = false;
    (async () => {
      try {
        const branchParam =
          selectedBranches === "all" ? null : selectedBranches;
        const cs = await repoCommits(
          selectedRepoPath!,
          branchParam,
          toWindowParam(windowDays),
        );
        if (!cancelled) setCommits(cs);
      } catch {}
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedBranches]);

  // Clear fresh-commit markers whenever the panel loses focus (= the user
  // has "seen" what was new). They re-populate as new commits arrive.
  useEffect(() => {
    function onBlur() {
      // Small delay so a tray-context-menu blur or a chip-dropdown click
      // doesn't wipe everything mid-interaction.
      window.setTimeout(() => {
        if (!document.hasFocus()) setFreshHashes(new Set());
      }, 200);
    }
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // ----- ESC layer cascade: chip → expansion (in Timeline) → single-repo → hide panel -----
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
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
  }, [openChip, singleMode]);

  const authors: AuthorTally[] = useMemo(() => {
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

  const filteredCommits = useMemo(() => {
    if (!commits) return null;
    if (selectedAuthors === "all") return commits;
    const set = new Set(selectedAuthors);
    return commits.filter((c) => set.has(c.author));
  }, [commits, selectedAuthors]);

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
      <header className="panel-header" onPointerDown={startDrag}>
        <img
          src="/icon.png"
          alt=""
          className="panel-header-icon"
          draggable={false}
        />
        <h1>gitwink</h1>
        <div className="header-chips">
          <RepoChip
            open={openChip === "repo"}
            onToggle={() => setOpenChip(openChip === "repo" ? null : "repo")}
            onClose={() => setOpenChip(null)}
            repos={allRepos}
            pinned={pinnedRepos}
            selectedPath={selectedRepoPath}
            onSelect={setSelectedRepoPath}
            onTogglePin={togglePin}
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
              onChange={setSelectedBranches}
            />
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
        {filteredCommits == null ? (
          <p className="panel-empty">Loading commits…</p>
        ) : (
          <Timeline
            key={singleMode ? `single:${selectedRepoPath}` : "all"}
            commits={filteredCommits}
            mode={singleMode ? "single" : "all"}
            onSelectRepo={singleMode ? undefined : setSelectedRepoPath}
            branches={singleMode ? branches : undefined}
            freshHashes={freshHashes}
          />
        )}
      </section>
    </main>
  );
}

export default App;
