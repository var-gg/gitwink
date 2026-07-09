import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import {
  listExcludedRepos,
  onRepoCatalogChanged,
  restoreRepo,
} from "../../lib/ipc";
import type { ExcludedRepo } from "../../types";
import "./ExcludedReposSection.css";

export function ExcludedReposSection() {
  const [repos, setRepos] = useState<ExcludedRepo[]>([]);
  const [loading, setLoading] = useState(true);
  const [restoringPath, setRestoringPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refreshSequence = useRef(0);

  const refresh = useCallback(async () => {
    const sequence = ++refreshSequence.current;
    try {
      const next = await listExcludedRepos();
      if (sequence !== refreshSequence.current) return;
      setRepos(next);
      setError(null);
    } catch (err) {
      if (sequence !== refreshSequence.current) return;
      setError(errorMessage(err, "Couldn't load excluded repositories"));
    } finally {
      if (sequence === refreshSequence.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void refresh();
    void onRepoCatalogChanged(() => void refresh()).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);

  async function restore(repo: ExcludedRepo) {
    if (restoringPath) return;
    setRestoringPath(repo.path);
    setError(null);
    try {
      await restoreRepo(repo.path);
      await refresh();
    } catch (err) {
      setError(errorMessage(err, `Couldn't restore ${repo.name}`));
    } finally {
      setRestoringPath(null);
    }
  }

  return (
    <section className="settings-section">
      <h2 className="settings-section-title">Repositories</h2>
      <p className="settings-hint settings-exclusions-intro">
        Excluded repositories are ignored by discovery and removed from the
        panel, but their folders, pin state and branch preferences stay intact.
      </p>

      {loading ? (
        <p className="settings-exclusions-empty">Loading…</p>
      ) : repos.length === 0 ? (
        <p className="settings-exclusions-empty">No excluded repositories.</p>
      ) : (
        <div className="settings-exclusions-list">
          {repos.map((repo) => {
            const restoring = restoringPath === repo.path;
            return (
              <div key={repo.path} className="settings-exclusion-row">
                <div className="settings-exclusion-copy">
                  <span className="settings-exclusion-name">{repo.name}</span>
                  <span className="settings-exclusion-path" title={repo.path}>
                    {repo.path}
                  </span>
                  <span className="settings-exclusion-meta">
                    {repo.folderAvailable
                      ? "Folder available"
                      : "Folder unavailable — restores as missing until it returns"}
                    {repo.excludedAt > 0 && (
                      <> · Excluded {formatTimestamp(repo.excludedAt)}</>
                    )}
                  </span>
                </div>
                <button
                  type="button"
                  className="settings-exclusion-restore"
                  disabled={restoringPath != null}
                  onClick={() => void restore(repo)}
                >
                  {restoring ? "Restoring…" : "Restore"}
                </button>
              </div>
            );
          })}
        </div>
      )}

      {error && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

function formatTimestamp(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}
