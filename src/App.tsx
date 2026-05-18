import { useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { AuthorsChip } from "./components/AuthorsChip";
import { RepoChip } from "./components/RepoChip";
import { Timeline } from "./components/Timeline";
import { TimeRangeChip } from "./components/TimeRangeChip";
import {
  discoverRepos,
  getPinnedRepos,
  listRecentCommitsCached,
  listRepos,
  onScanComplete,
  onScanProgress,
  onTimelineRepoFill,
  recentCommits,
  setPinnedRepos as savePinnedRepos,
} from "./lib/ipc";
import type {
  AuthorTally,
  CommitSummary,
  Repo,
  WindowDays,
} from "./types";
import "./styles.css";

const TIMELINE_MAX = 50;

function startDrag(e: React.PointerEvent<HTMLElement>) {
  if (e.button !== 0) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest("button, input, [data-no-drag]")) return;
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
  const [openChip, setOpenChip] = useState<"repo" | "time" | "authors" | null>(
    null,
  );

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
        setCommits((prev) => mergeCommits(prev ?? [], p.commits));
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

  useEffect(() => {
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
  }, [windowDays]);

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
    let f = commits;
    if (selectedRepoPath) f = f.filter((c) => c.repoPath === selectedRepoPath);
    if (selectedAuthors !== "all") {
      const set = new Set(selectedAuthors);
      f = f.filter((c) => set.has(c.author));
    }
    return f;
  }, [commits, selectedRepoPath, selectedAuthors]);

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
    <main className="panel">
      <header className="panel-header" onPointerDown={startDrag}>
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
      </header>
      <section className="panel-body">
        {filteredCommits == null ? (
          <p className="panel-empty">Loading commits…</p>
        ) : (
          <Timeline commits={filteredCommits} />
        )}
      </section>
    </main>
  );
}

export default App;
