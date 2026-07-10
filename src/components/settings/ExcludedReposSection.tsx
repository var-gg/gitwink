import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import {
  listExcludedRepos,
  inspectReplacementRepo,
  onRepoCatalogChanged,
  replaceExcludedRepo,
  restoreRepo,
} from "../../lib/ipc";
import type { ExcludedRepo, ReplacementConfirmation } from "../../types";
import "./ExcludedReposSection.css";

export function ExcludedReposSection() {
  const [repos, setRepos] = useState<ExcludedRepo[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<{
    path: string;
    kind: "restore" | "inspect" | "replace";
  } | null>(null);
  const [confirmation, setConfirmation] =
    useState<ReplacementConfirmation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const refreshSequence = useRef(0);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const restoreButtons = useRef(new Map<string, HTMLButtonElement>());
  const inspectButtons = useRef(new Map<string, HTMLButtonElement>());
  const focusAfterIndex = useRef<number | null>(null);
  const focusInspectPath = useRef<string | null>(null);

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
    void onRepoCatalogChanged(() => {
      setConfirmation(null);
      void refresh();
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);

  useEffect(() => {
    const discardHiddenConfirmation = () => {
      if (document.visibilityState === "hidden") setConfirmation(null);
    };
    document.addEventListener("visibilitychange", discardHiddenConfirmation);
    return () =>
      document.removeEventListener(
        "visibilitychange",
        discardHiddenConfirmation,
      );
  }, []);

  useEffect(() => {
    const index = focusAfterIndex.current;
    if (index == null) return;
    focusAfterIndex.current = null;
    const next = repos[Math.min(index, Math.max(0, repos.length - 1))];
    requestAnimationFrame(() => {
      if (next) restoreButtons.current.get(next.path)?.focus();
      else headingRef.current?.focus();
    });
  }, [repos]);

  useEffect(() => {
    const path = focusInspectPath.current;
    if (!path || confirmation != null) return;
    focusInspectPath.current = null;
    requestAnimationFrame(() => inspectButtons.current.get(path)?.focus());
  }, [confirmation]);

  async function restore(repo: ExcludedRepo) {
    if (busy) return;
    setBusy({ path: repo.path, kind: "restore" });
    setError(null);
    try {
      await restoreRepo(repo.path);
      focusAfterIndex.current = repos.findIndex((item) => item.path === repo.path);
      setConfirmation(null);
      setAnnouncement(`${repo.name} restored.`);
      await refresh();
    } catch (err) {
      const message = errorMessage(err, `Couldn't restore ${repo.name}`);
      if (message.includes("REPOSITORY_IDENTITY_MISMATCH")) {
        try {
          setBusy({ path: repo.path, kind: "inspect" });
          setConfirmation(await inspectReplacementRepo(repo.path));
          setError(
            "This folder now contains a different Git repository. Review the replacement details before confirming.",
          );
        } catch (inspectError) {
          setConfirmation(null);
          setError(
            errorMessage(inspectError, `Couldn't inspect ${repo.name}`),
          );
        }
      } else {
        setError(message);
      }
    } finally {
      setBusy(null);
    }
  }

  async function inspectReplacement(repo: ExcludedRepo) {
    if (busy) return;
    setBusy({ path: repo.path, kind: "inspect" });
    setError(null);
    try {
      setConfirmation(await inspectReplacementRepo(repo.path));
    } catch (err) {
      setConfirmation(null);
      setError(errorMessage(err, `Couldn't inspect ${repo.name}`));
    } finally {
      setBusy(null);
    }
  }

  async function replace(repo: ExcludedRepo) {
    if (busy || confirmation?.canonicalPath !== repo.path) return;
    setBusy({ path: repo.path, kind: "replace" });
    setError(null);
    try {
      await replaceExcludedRepo(confirmation);
      focusAfterIndex.current = repos.findIndex((item) => item.path === repo.path);
      setConfirmation(null);
      setAnnouncement(`${repo.name} replaced with the repository currently in its folder.`);
      await refresh();
    } catch (err) {
      const message = errorMessage(
        err,
        `Couldn't use the current ${repo.name} repository`,
      );
      if (message.includes("REPLACEMENT_CONFIRMATION_EXPIRED")) {
        focusInspectPath.current = repo.path;
        setConfirmation(null);
        setError(
          "The repository changed after you reviewed it. Review the current repository again before confirming.",
        );
      } else {
        setError(message);
      }
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="settings-section">
      <h2
        ref={headingRef}
        className="settings-section-title"
        tabIndex={-1}
      >
        Repositories
      </h2>
      <p className="settings-hint settings-exclusions-intro">
        Excluded repositories are ignored by discovery and removed from the
        panel, but their folders, pin state and branch preferences stay intact.
      </p>

      {loading ? (
        <p className="settings-exclusions-empty">Loading…</p>
      ) : repos.length === 0 ? (
        <p className="settings-exclusions-empty">No excluded repositories.</p>
      ) : (
        <div className="settings-exclusions-list" role="list">
          {repos.map((repo, index) => {
            const rowBusy = busy?.path === repo.path;
            const restoring = rowBusy && busy.kind === "restore";
            const inspecting = rowBusy && busy.kind === "inspect";
            const replacing = rowBusy && busy.kind === "replace";
            const confirmingReplace = confirmation?.canonicalPath === repo.path;
            const warningId = `replacement-warning-${index}`;
            return (
              <div
                key={repo.path}
                className="settings-exclusion-row"
                role="listitem"
              >
                <div className="settings-exclusion-copy">
                  <span className="settings-exclusion-name">{repo.name}</span>
                  <span className="settings-exclusion-path" title={repo.path}>
                    {repo.path}
                  </span>
                  <span className="settings-exclusion-meta">
                    {repo.folderAvailable == null
                      ? "Folder availability is checked on restore"
                      : repo.folderAvailable
                        ? "Folder available"
                        : "Folder unavailable — restores as missing until it returns"}
                    {repo.excludedAt > 0 && (
                      <> · Excluded {formatTimestamp(repo.excludedAt)}</>
                    )}
                  </span>
                  {confirmingReplace && (
                    <span
                      id={warningId}
                      className="settings-exclusion-replace-warning"
                    >
                      Reviewed: <strong>{confirmation.repoName}</strong>. The
                      confirmation is valid only while this exact repository
                      and exclusion lifecycle remain unchanged. {" "}
                      This forgets gitwink's old cache, pin and branch choices
                      for this path, then registers the Git repository that is
                      there now. The folder itself is never changed.
                    </span>
                  )}
                </div>
                <div className="settings-exclusion-actions">
                  <button
                    ref={(node) => {
                      if (node) restoreButtons.current.set(repo.path, node);
                      else restoreButtons.current.delete(repo.path);
                    }}
                    type="button"
                    className="settings-exclusion-restore"
                    aria-label={`Restore ${repo.name}`}
                    disabled={busy != null}
                    onClick={() => void restore(repo)}
                  >
                    {restoring ? "Restoring…" : "Restore"}
                  </button>
                  {confirmingReplace ? (
                    <>
                      <button
                        type="button"
                        className="settings-exclusion-replace confirm"
                        aria-label={`Confirm replacing ${repo.name} with ${confirmation.repoName}`}
                        aria-describedby={warningId}
                        disabled={busy != null}
                        onClick={() => void replace(repo)}
                      >
                        {replacing ? "Replacing…" : "Confirm replace"}
                      </button>
                      <button
                        type="button"
                        className="settings-exclusion-cancel"
                        aria-label={`Cancel replacing ${repo.name}`}
                        disabled={busy != null}
                        onClick={() => {
                          focusInspectPath.current = repo.path;
                          setConfirmation(null);
                          setError(null);
                        }}
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button
                      ref={(node) => {
                        if (node) inspectButtons.current.set(repo.path, node);
                        else inspectButtons.current.delete(repo.path);
                      }}
                      type="button"
                      className="settings-exclusion-replace"
                      aria-label={`Use the current repository in ${repo.name}'s folder`}
                      disabled={busy != null}
                      onClick={() => void inspectReplacement(repo)}
                    >
                      {inspecting ? "Inspecting…" : "Use current repo…"}
                    </button>
                  )}
                </div>
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
      <p className="sr-only" role="status" aria-live="polite">
        {announcement}
      </p>
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
