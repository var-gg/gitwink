import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { Timeline } from "./components/Timeline";
import {
  discoverRepos,
  listRepos,
  onScanComplete,
  onScanProgress,
  recentCommits,
} from "./lib/ipc";
import type { CommitSummary } from "./types";
import "./styles.css";

function startDrag(e: React.MouseEvent) {
  if (e.buttons !== 1) return;
  void getCurrentWindow().startDragging();
}

function App() {
  const [repoCount, setRepoCount] = useState<number | null>(null);
  const [scanning, setScanning] = useState(false);
  const [commits, setCommits] = useState<CommitSummary[] | null>(null);

  useEffect(() => {
    let mounted = true;
    let unP: UnlistenFn | undefined;
    let unC: UnlistenFn | undefined;

    async function refreshCommits() {
      try {
        const c = await recentCommits();
        if (mounted) setCommits(c);
      } catch {
        // surface as empty for v0.1
        if (mounted) setCommits([]);
      }
    }

    (async () => {
      try {
        const cached = await listRepos();
        if (!mounted) return;
        setRepoCount(cached.length);
        if (cached.length > 0) {
          void refreshCommits();
        }
      } catch {
        // First-run: cache file may not exist yet.
      }

      unP = await onScanProgress((p) => {
        if (mounted) setRepoCount(p.found);
      });
      unC = await onScanComplete(async (p) => {
        if (!mounted) return;
        setRepoCount(p.count);
        setScanning(false);
        await refreshCommits();
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
    };
  }, []);

  let status: string;
  if (repoCount == null) {
    status = "Loading…";
  } else if (scanning) {
    status = `Scanning… ${repoCount} ${repoCount === 1 ? "repo" : "repos"}`;
  } else {
    status = `${repoCount} ${repoCount === 1 ? "repository" : "repositories"}`;
  }

  return (
    <main className="panel">
      <header className="panel-header" onMouseDown={startDrag}>
        <h1>gitwink</h1>
        <span className="panel-status">{status}</span>
      </header>
      <section className="panel-body">
        {commits == null ? (
          <p className="panel-empty">Loading commits…</p>
        ) : (
          <Timeline commits={commits} />
        )}
      </section>
    </main>
  );
}

export default App;
