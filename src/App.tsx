import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

import {
  discoverRepos,
  listRepos,
  onScanComplete,
  onScanProgress,
} from "./lib/ipc";
import "./styles.css";

function startDrag(e: React.MouseEvent) {
  if (e.buttons !== 1) return;
  void getCurrentWindow().startDragging();
}

function App() {
  const [count, setCount] = useState<number | null>(null);
  const [scanning, setScanning] = useState(false);

  useEffect(() => {
    let mounted = true;
    let unP: UnlistenFn | undefined;
    let unC: UnlistenFn | undefined;

    (async () => {
      try {
        const cached = await listRepos();
        if (!mounted) return;
        setCount(cached.length);
      } catch {
        // First-run: cache file may not exist yet. Fine.
      }

      unP = await onScanProgress((p) => {
        if (mounted) setCount(p.found);
      });
      unC = await onScanComplete((p) => {
        if (!mounted) return;
        setCount(p.count);
        setScanning(false);
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
  if (count == null) {
    status = "Loading…";
  } else if (scanning) {
    status = `Scanning… ${count} ${count === 1 ? "repo" : "repos"} found`;
  } else {
    status = `Found ${count} ${count === 1 ? "repository" : "repositories"}`;
  }

  return (
    <main className="panel">
      <header className="panel-header" onMouseDown={startDrag}>
        <h1>gitwink</h1>
        <span className="panel-status">v0.1 — bootstrapping</span>
      </header>
      <section className="panel-body">
        <p className="panel-empty">{status}</p>
      </section>
    </main>
  );
}

export default App;
