import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { updateInstall, updateSkip, updateSnooze } from "../lib/ipc";
import type { UpdateStatePayload } from "../types";
import { announceModalOpen } from "./overlayLifecycle";

interface UpdateModalProps {
  state: UpdateStatePayload;
  onClose: () => void;
}

/** Panel-overlay modal for the self-updater. Summoned by the backend
 * (tray "Update available" / a manual check) — never auto-pops. */
export function UpdateModal({ state, onClose }: UpdateModalProps) {
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const update = state.available;
  const visible = state.scoop || update != null;

  useLayoutEffect(() => {
    if (visible) announceModalOpen();
  }, [visible]);

  // Scoop installs: no in-app update path — point the user at the CLI. The
  // badge + version still show (detection is the same), only the action is a
  // command rather than an in-app install.
  if (state.scoop) {
    return (
      <Backdrop onClose={onClose} focusEpoch={0}>
        <header className="update-modal-header">
          <span className="update-modal-title">
            {update ? "Update available" : "Update gitwink"}
          </span>
          {update && (
            <span className="update-modal-version">v{update.version}</span>
          )}
        </header>
        <div className="update-modal-changelog">
          This copy of gitwink was installed with Scoop, which manages its own
          updates. Run <code>scoop update gitwink</code> in a terminal to get{" "}
          {update ? `v${update.version}` : "the latest version"}.
        </div>
        <div className="update-modal-actions">
          <button
            type="button"
            className="update-btn update-btn-primary"
            onClick={onClose}
          >
            Got it
          </button>
        </div>
      </Backdrop>
    );
  }

  // Defensive: backend only emits show-modal with an update or scoop=true.
  if (!update) return null;

  async function install() {
    setInstalling(true);
    setError(null);
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
            : "Update failed",
      );
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
        <span className="update-modal-title">Update available</span>
        <span className="update-modal-version">v{update.version}</span>
      </header>
      <div className="update-modal-changelog">
        {update.notes.trim() || "No release notes provided for this version."}
      </div>
      {error && <div className="update-modal-error">{error}</div>}
      <div className="update-modal-actions">
        <button
          type="button"
          className="update-btn update-btn-primary"
          onClick={() => void install()}
          disabled={installing}
        >
          {installing ? "Updating…" : "Update now"}
        </button>
        <button
          type="button"
          className="update-btn"
          onClick={() => void later()}
          disabled={installing}
        >
          Later
        </button>
        <button
          type="button"
          className="update-btn update-btn-quiet"
          onClick={() => void skip()}
          disabled={installing}
        >
          Skip v{update.version}
        </button>
      </div>
    </Backdrop>
  );
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
        aria-label="gitwink update"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
