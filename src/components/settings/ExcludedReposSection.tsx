import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useLingui } from "@lingui/react";

import {
  listExcludedRepos,
  inspectReplacementRepo,
  onRepoCatalogChanged,
  replaceExcludedRepo,
  restoreRepo,
} from "../../lib/ipc";
import type { ExcludedRepo, ReplacementConfirmation } from "../../types";
import { repositoryMessages as m } from "../../i18n/messages/repositories";
import { translate } from "../../i18n/translate";
import "./ExcludedReposSection.css";

export function ExcludedReposSection() {
  const { i18n } = useLingui();
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
      setError(errorMessage(err, i18n._(m.loadError)));
    } finally {
      if (sequence === refreshSequence.current) setLoading(false);
    }
  }, [i18n]);

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
      setAnnouncement(
        translate(i18n, m.restoredAnnouncement, { repo: repo.name }),
      );
      await refresh();
    } catch (err) {
      const message = errorMessage(
        err,
        translate(i18n, m.restoreError, { repo: repo.name }),
      );
      if (message.includes("REPOSITORY_IDENTITY_MISMATCH")) {
        try {
          setBusy({ path: repo.path, kind: "inspect" });
          setConfirmation(await inspectReplacementRepo(repo.path));
          setError(
            i18n._(m.identityMismatch),
          );
        } catch (inspectError) {
          setConfirmation(null);
          setError(
            errorMessage(
              inspectError,
              translate(i18n, m.inspectError, { repo: repo.name }),
            ),
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
      setError(
        errorMessage(
          err,
          translate(i18n, m.inspectError, { repo: repo.name }),
        ),
      );
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
      setAnnouncement(
        translate(i18n, m.replacedAnnouncement, { repo: repo.name }),
      );
      await refresh();
    } catch (err) {
      const message = errorMessage(
        err,
        translate(i18n, m.replaceError, { repo: repo.name }),
      );
      if (message.includes("REPLACEMENT_CONFIRMATION_EXPIRED")) {
        focusInspectPath.current = repo.path;
        setConfirmation(null);
        setError(
          i18n._(m.confirmationExpired),
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
        {i18n._(m.title)}
      </h2>
      <p className="settings-hint settings-exclusions-intro">
        {i18n._(m.intro)}
      </p>

      {loading ? (
        <p className="settings-exclusions-empty">{i18n._(m.loading)}</p>
      ) : repos.length === 0 ? (
        <p className="settings-exclusions-empty">{i18n._(m.empty)}</p>
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
                      ? i18n._(m.folderCheckedOnRestore)
                      : repo.folderAvailable
                        ? i18n._(m.folderAvailable)
                        : i18n._(m.folderUnavailable)}
                    {repo.excludedAt > 0 && (
                      <>
                        {" · "}
                        {translate(i18n, m.excludedAt, {
                          when: formatTimestamp(
                            repo.excludedAt,
                            settingsLocale(i18n.locale),
                          ),
                        })}
                      </>
                    )}
                  </span>
                  {confirmingReplace && (
                    <span
                      id={warningId}
                      className="settings-exclusion-replace-warning"
                    >
                      {translate(i18n, m.replacementWarning, {
                        repo: confirmation.repoName,
                      })}
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
                    aria-label={translate(i18n, m.restoreAria, {
                      repo: repo.name,
                    })}
                    disabled={busy != null}
                    onClick={() => void restore(repo)}
                  >
                    {restoring ? i18n._(m.restoring) : i18n._(m.restore)}
                  </button>
                  {confirmingReplace ? (
                    <>
                      <button
                        type="button"
                        className="settings-exclusion-replace confirm"
                        aria-label={translate(i18n, m.confirmReplaceAria, {
                          oldRepo: repo.name,
                          newRepo: confirmation.repoName,
                        })}
                        aria-describedby={warningId}
                        disabled={busy != null}
                        onClick={() => void replace(repo)}
                      >
                        {replacing
                          ? i18n._(m.replacing)
                          : i18n._(m.confirmReplace)}
                      </button>
                      <button
                        type="button"
                        className="settings-exclusion-cancel"
                        aria-label={translate(i18n, m.cancelReplaceAria, {
                          repo: repo.name,
                        })}
                        disabled={busy != null}
                        onClick={() => {
                          focusInspectPath.current = repo.path;
                          setConfirmation(null);
                          setError(null);
                        }}
                      >
                        {i18n._(m.cancel)}
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
                      aria-label={translate(i18n, m.useCurrentAria, {
                        repo: repo.name,
                      })}
                      disabled={busy != null}
                      onClick={() => void inspectReplacement(repo)}
                    >
                      {inspecting ? i18n._(m.inspecting) : i18n._(m.inspect)}
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

function formatTimestamp(unixSeconds: number, locale: string): string {
  return new Date(unixSeconds * 1000).toLocaleString(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

function settingsLocale(locale: string): string {
  return locale === "ko" ? "ko-KR" : locale === "ja" ? "ja-JP" : "en-US";
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}
