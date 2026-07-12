import { useLingui } from "@lingui/react";

import { panelMessages as m } from "../../i18n/messages/panel";

interface FirstRunTipProps {
  onDismiss: () => void;
}

export function FirstRunTip({ onDismiss }: FirstRunTipProps) {
  const { i18n } = useLingui();
  return (
    <div className="firstrun-tip">
      <span className="firstrun-tip-text">
        <kbd>/</kbd> {i18n._(m.firstRunTip)}
      </span>
      <button
        type="button"
        className="firstrun-tip-dismiss"
        aria-label={i18n._(m.dismissTip)}
        title={i18n._(m.gotIt)}
        onClick={onDismiss}
      >
        ✕
      </button>
    </div>
  );
}

interface EmptyDropPanelProps {
  scanning: boolean;
  addError: string | null;
  onBrowse: () => void;
  onPaste: () => void;
  pasteShortcut: string;
  pasting: boolean;
}

/** First-paint state for a fresh machine. The explicit-add path stays primary
 * and the caller keeps the panel sticky while a file manager is in front. */
export function EmptyDropPanel({
  scanning,
  addError,
  onBrowse,
  onPaste,
  pasteShortcut,
  pasting,
}: EmptyDropPanelProps) {
  const { i18n } = useLingui();
  return (
    <div className="empty-drop">
      <div className="empty-drop-icon" aria-hidden="true">
        📂
      </div>
      <div className="empty-drop-title">{i18n._(m.dropTitle)}</div>
      <div className="empty-drop-sub">{i18n._(m.pastePath)}</div>
      <div className="empty-drop-actions">
        <button
          type="button"
          className="add-repo-btn empty-drop-btn"
          onClick={onBrowse}
        >
          {i18n._(m.browseFolder)}
        </button>
        <PasteRepoButton
          className="empty-drop-paste-btn"
          onPaste={onPaste}
          pasteShortcut={pasteShortcut}
          pasting={pasting}
        />
      </div>
      {scanning && (
        <div className="empty-drop-status">{i18n._(m.scanningRepos)}</div>
      )}
      {addError && <div className="empty-drop-error">{addError}</div>}
    </div>
  );
}

interface PanelFooterProps {
  addError: string | null;
  onAddRepo: () => void;
  onPaste: () => void;
  pasteShortcut: string;
  pasting: boolean;
}

interface PasteRepoButtonProps {
  className?: string;
  onPaste: () => void;
  pasteShortcut: string;
  pasting: boolean;
}

export function PasteRepoButton({
  className = "",
  onPaste,
  pasteShortcut,
  pasting,
}: PasteRepoButtonProps) {
  const { i18n } = useLingui();
  const title = i18n._({
    ...m.pasteTitle,
    values: { shortcut: pasteShortcut },
  });
  return (
    <button
      type="button"
      className={`paste-repo-btn ${className}`.trim()}
      title={title}
      aria-label={title}
      disabled={pasting}
      onClick={onPaste}
    >
      <span>{pasting ? i18n._(m.pasteBusy) : i18n._(m.pasteShort)}</span>
      <kbd>{pasteShortcut}</kbd>
    </button>
  );
}

export function PanelFooter({
  addError,
  onAddRepo,
  onPaste,
  pasteShortcut,
  pasting,
}: PanelFooterProps) {
  const { i18n } = useLingui();
  return (
    <div className="panel-footer-hint">
      <div className="panel-footer-actions">
        <button type="button" className="add-repo-btn" onClick={onAddRepo}>
          {i18n._(m.addRepo)}
        </button>
        <span className="panel-footer-or">{i18n._(m.or)}</span>
        <PasteRepoButton
          onPaste={onPaste}
          pasteShortcut={pasteShortcut}
          pasting={pasting}
        />
      </div>
      {addError && (
        <div className="panel-footer-hint-error" role="status" title={addError}>
          {addError}
        </div>
      )}
    </div>
  );
}
