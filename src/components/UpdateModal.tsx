import { useState } from "react";

import { updateInstall, updateSkip, updateSnooze } from "../lib/ipc";
import type { UpdateStatePayload } from "../types";

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

  // Scoop installs: no in-app update path — point the user at the CLI.
  if (state.scoop) {
    return (
      <Backdrop onClose={onClose}>
        <header className="update-modal-header">
          <span className="update-modal-title">Update gitwink</span>
        </header>
        <div className="update-modal-changelog">
          This copy of gitwink was installed with Scoop, which manages its
          own updates. Run <code>scoop update gitwink</code> in a terminal
          to get the latest version.
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
    <Backdrop onClose={installing ? undefined : onClose}>
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
}

function Backdrop({ children, onClose }: BackdropProps) {
  return (
    <div
      className="update-modal-backdrop"
      onClick={onClose}
      data-no-drag
    >
      <div
        className="update-modal"
        role="dialog"
        aria-modal="true"
        aria-label="gitwink update"
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
