import { useLingui } from "@lingui/react";

import { panelMessages as m } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";
import type { HistoryIndexStatus } from "../../types";

export interface ControlNotice {
  tone: "ready" | "error";
  source: string;
  detail: string;
}

interface PanelNoticesProps {
  showFacetError: boolean;
  onRetryFacets: () => void;
  historyDemanded: boolean;
  historyIndexError: boolean;
  historyIndexStatus: HistoryIndexStatus | null;
  onRetryHistoryIndex: () => void;
  showAutoFetchNotice: boolean;
  onOpenSettings: () => void;
  onDismissAutoFetchNotice: () => void;
  controlNotice: ControlNotice | null;
  onDismissControlNotice: () => void;
}

/** Status surfaces between the fixed header and scrollable body. They share
 * placement, not domain state: App still owns retry/dismiss effects and this
 * component remains a deterministic projection of that state. */
export function PanelNotices({
  showFacetError,
  onRetryFacets,
  historyDemanded,
  historyIndexError,
  historyIndexStatus,
  onRetryHistoryIndex,
  showAutoFetchNotice,
  onOpenSettings,
  onDismissAutoFetchNotice,
  controlNotice,
  onDismissControlNotice,
}: PanelNoticesProps) {
  const { i18n } = useLingui();

  return (
    <>
      {showFacetError && (
        <div className="facet-error-notice" role="status">
          <span>{i18n._(m.facetsError)}</span>
          <button
            type="button"
            className="facet-error-retry"
            onClick={onRetryFacets}
          >
            {i18n._(m.retry)}
          </button>
        </div>
      )}

      {historyDemanded &&
        (historyIndexError || !historyIndexStatus?.allComplete) && (
          <div className="history-index-notice" role="status" aria-live="polite">
            <span className="history-index-notice-text">
              {historyStatusText(i18n, historyIndexError, historyIndexStatus)}
            </span>
            {(historyIndexError ||
              (historyIndexStatus?.settled &&
                !historyIndexStatus.allComplete)) && (
              <button
                type="button"
                className="history-index-retry"
                onClick={onRetryHistoryIndex}
              >
                {i18n._(m.retry)}
              </button>
            )}
          </div>
        )}

      {showAutoFetchNotice && (
        <div className="autofetch-notice" role="status">
          <span className="autofetch-notice-text">
            {i18n._(m.autoFetchDisclosure)}
          </span>
          <span className="autofetch-notice-actions">
            <button
              type="button"
              className="autofetch-notice-link"
              onClick={onOpenSettings}
            >
              {i18n._(m.settings)}
            </button>
            <button
              type="button"
              className="autofetch-notice-dismiss"
              onClick={onDismissAutoFetchNotice}
            >
              {i18n._(m.gotIt)}
            </button>
          </span>
        </div>
      )}

      {controlNotice && (
        <div
          className={`control-handoff ${controlNotice.tone}`}
          role={controlNotice.tone === "error" ? "alert" : "status"}
          aria-live={controlNotice.tone === "error" ? "assertive" : "polite"}
        >
          <span
            className="control-handoff-text"
            title={`${controlNotice.source}: ${controlNotice.detail}`}
          >
            <strong>
              {translate(
                i18n,
                controlNotice.tone === "ready" ? m.controlOpened : m.controlFailed,
                { source: controlNotice.source },
              )}
            </strong>
            <span aria-hidden="true"> · </span>
            {controlNotice.detail}
          </span>
          <button
            type="button"
            className="control-handoff-dismiss"
            aria-label={i18n._(m.controlDismiss)}
            onClick={onDismissControlNotice}
          >
            ✕
          </button>
        </div>
      )}
    </>
  );
}

function historyStatusText(
  i18n: ReturnType<typeof useLingui>["i18n"],
  failed: boolean,
  status: HistoryIndexStatus | null,
): string {
  if (failed) return i18n._(m.historyPaused);
  if (!status) return i18n._(m.historyPreparing);
  if (!status.settled) {
    return translate(i18n, m.historyIndexing, {
      published: status.publishedRepos,
      total: status.totalRepos,
      commits: status.commitsScanned.toLocaleString(i18n.locale),
    });
  }
  const failedSuffix =
    status.failedRepos > 0
      ? translate(i18n, m.historyFailed, { count: status.failedRepos })
      : "";
  const offlineSuffix =
    status.unavailableRepos > 0
      ? translate(i18n, m.historyOffline, {
          count: status.unavailableRepos,
        })
      : "";
  return translate(i18n, m.historyAvailable, {
    published: status.publishedRepos,
    total: status.totalRepos,
    failed: failedSuffix,
    offline: offlineSuffix,
  });
}
