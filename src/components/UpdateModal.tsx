import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { I18n } from "@lingui/core";
import { useLingui } from "@lingui/react";

import {
  onUpdateProgress,
  updateInstall,
  updateSkip,
  updateSnooze,
} from "../lib/ipc";
import { shouldPresentUpdate } from "../lib/updatePolicy";
import {
  updateMessages as m,
  updateProviderLabel,
} from "../i18n/messages/updates";
import { translate } from "../i18n/translate";
import type {
  AvailableUpdate,
  UpdateProgress,
  UpdateStatePayload,
} from "../types";
import { announceModalOpen } from "./overlayLifecycle";

interface UpdateModalProps {
  state: UpdateStatePayload;
  onClose: () => void;
}

/** Panel-overlay modal for the self-updater. Summoned by the backend
 * (tray "Update available" / a manual check) — never auto-pops. */
export function UpdateModal({ state, onClose }: UpdateModalProps) {
  const { i18n } = useLingui();
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  const update = state.available;
  const visible = shouldPresentUpdate(state);

  useLayoutEffect(() => {
    if (visible) announceModalOpen();
  }, [visible]);

  useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;
    void onUpdateProgress((next) => {
      if (mounted) setProgress(next);
    })
      .then((stop) => {
        if (mounted) unlisten = stop;
        else stop();
      })
      .catch(() => {});
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, []);

  // Defensive: the backend only emits show-modal after publishing an update.
  if (!update) return null;

  const installAction = state.policy.installAction;

  // Externally managed installs still share discovery/badges, but the final
  // action remains owned by the package manager described by the policy.
  if (installAction.kind === "externalCommand") {
    return (
      <Backdrop onClose={onClose} focusEpoch={0}>
        <header className="update-modal-header">
          <span className="update-modal-title">{i18n._(m.available)}</span>
          <span className="update-modal-version">v{update.version}</span>
        </header>
        <div className="update-modal-changelog">
          {translate(i18n, m.managedExternally, {
            provider: updateProviderLabel(i18n, installAction.provider),
            command: installAction.command,
            version: update.version,
          })}
        </div>
        <div className="update-modal-actions">
          <button
            type="button"
            className="update-btn update-btn-primary"
            onClick={onClose}
          >
            {i18n._(m.gotIt)}
          </button>
        </div>
      </Backdrop>
    );
  }

  async function install() {
    setInstalling(true);
    setError(null);
    setProgress({ phase: "preparing", progress: 0 });
    try {
      await updateInstall();
      // On success the app relaunches before this resolves; reaching
      // here means the install failed pre-relaunch.
      setInstalling(false);
    } catch (e) {
      setError(
        typeof e === "string"
          ? e
          : e instanceof Error
            ? e.message
            : i18n._(m.failed),
      );
      setProgress(null);
      setInstalling(false);
    }
  }

  async function later() {
    await updateSnooze();
    onClose();
  }

  async function skip() {
    await updateSkip();
    onClose();
  }

  return (
    <Backdrop
      onClose={installing ? undefined : onClose}
      focusEpoch={installing ? 1 : 0}
    >
      <header className="update-modal-header">
        <span className="update-modal-title">
          {installAction.kind === "inApp"
            ? translate(i18n, m.inAppTitle, {
                provider: updateProviderLabel(i18n, installAction.provider),
              })
            : i18n._(m.available)}
        </span>
        <span className="update-modal-version">v{update.version}</span>
      </header>
      <div className="update-modal-changelog">
        {update.notes.trim() || i18n._(m.noNotes)}
      </div>
      {installing && progress && (
        <div className="update-modal-progress" aria-live="polite">
          <div className="update-modal-progress-row">
            <span>
              {updateProgressLabel(
                progress,
                updateProviderLabel(i18n, installAction.provider),
                i18n,
              )}
            </span>
            {progress.phase === "downloading" && (
              <span>{Math.round(progress.progress * 100)}%</span>
            )}
          </div>
          <progress
            max={1}
            value={
              progress.phase === "preparing" || progress.phase === "waiting"
                ? undefined
                : progress.progress
            }
            aria-label={i18n._(m.progress)}
          />
        </div>
      )}
      {error && (
        <div className="update-modal-error" role="alert">
          {error}
        </div>
      )}
      <div className="update-modal-actions">
        <button
          type="button"
          className="update-btn update-btn-primary"
          onClick={() => void install()}
          disabled={installing}
        >
          {installing
            ? updateProgressLabel(
                progress,
                updateProviderLabel(i18n, installAction.provider),
                i18n,
              )
            : i18n._(m.updateNow)}
        </button>
        {canDeferUpdate(update) && (
          <>
            <button
              type="button"
              className="update-btn"
              onClick={() => void later()}
              disabled={installing}
            >
              {i18n._(m.later)}
            </button>
            <button
              type="button"
              className="update-btn update-btn-quiet"
              onClick={() => void skip()}
              disabled={installing}
            >
              {translate(i18n, m.skip, { version: update.version })}
            </button>
          </>
        )}
      </div>
    </Backdrop>
  );
}

export function updateProgressLabel(
  progress: UpdateProgress | null,
  providerLabel = "update provider",
  i18n?: I18n,
): string {
  switch (progress?.phase) {
    case "waiting":
      return i18n
        ? translate(i18n, m.waiting, { provider: providerLabel })
        : `Waiting for ${providerLabel}…`;
    case "downloading":
      return i18n ? i18n._(m.downloading) : "Downloading…";
    case "installing":
      return i18n ? i18n._(m.installing) : "Installing…";
    case "restarting":
      return i18n ? i18n._(m.restarting) : "Restarting…";
    default:
      return i18n ? i18n._(m.preparing) : "Preparing…";
  }
}

export function canDeferUpdate(
  update: Pick<AvailableUpdate, "mandatory">,
): boolean {
  return !update.mandatory;
}

interface BackdropProps {
  children: React.ReactNode;
  /** Click-outside / backdrop dismiss. Omit to disable (e.g. mid-install). */
  onClose?: () => void;
  /** Re-home focus when controls transition into a disabled/busy state. */
  focusEpoch: number;
}

/** Decide whether a modal Tab needs an explicit wrap target. `"dialog"`
 * covers the busy state where every action is disabled. */
export function modalTabDestination(
  focusableCount: number,
  activeIndex: number,
  shiftKey: boolean,
): number | "dialog" | null {
  if (focusableCount === 0) return "dialog";
  if (activeIndex < 0) return shiftKey ? focusableCount - 1 : 0;
  if (shiftKey && activeIndex === 0) return focusableCount - 1;
  if (!shiftKey && activeIndex === focusableCount - 1) return 0;
  return null;
}

function Backdrop({ children, onClose, focusEpoch }: BackdropProps) {
  const { i18n } = useLingui();
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(
    typeof document !== "undefined" &&
      document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null,
  );

  const focusInside = () => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const first = dialog.querySelector<HTMLElement>(
      'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    );
    (first ?? dialog).focus({ preventScroll: true });
  };

  useLayoutEffect(() => {
    focusInside();
  }, [focusEpoch]);

  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const dialog = dialogRef.current;
      if (!dialog || dialog.contains(event.target as Node)) return;
      focusInside();
    };
    document.addEventListener("focusin", onFocusIn);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      const target = restoreFocusRef.current;
      if (target?.isConnected && target !== document.body) {
        window.requestAnimationFrame(() => {
          if (target.isConnected) target.focus({ preventScroll: true });
        });
      }
    };
  }, []);

  return (
    <div
      className="update-modal-backdrop"
      onClick={onClose}
      onKeyDown={(event) => {
        // The modal is the sole keyboard owner. In particular, j/k/c must not
        // keep navigating the timeline beneath it while an update action has
        // focus.
        if (event.key === "Escape") {
          if (onClose) onClose();
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        if (event.key === "Tab") {
          const focusable = Array.from(
            dialogRef.current?.querySelectorAll<HTMLElement>(
              'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
            ) ?? [],
          );
          const target = modalTabDestination(
            focusable.length,
            focusable.indexOf(document.activeElement as HTMLElement),
            event.shiftKey,
          );
          if (target === "dialog") {
            // Installing disables every action. Keep focus on the dialog
            // itself rather than allowing Tab to escape to the panel below.
            dialogRef.current?.focus({ preventScroll: true });
            event.preventDefault();
          } else if (target != null) {
            focusable[target].focus({ preventScroll: true });
            event.preventDefault();
          }
        }
        event.stopPropagation();
      }}
      data-no-drag
    >
      <div
        ref={dialogRef}
        className="update-modal"
        role="dialog"
        aria-modal="true"
        aria-label={i18n._(m.dialog)}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
