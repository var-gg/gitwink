import type { UpdateCheckNotice, UpstreamStatus } from "../../types";
import type { RepoFetchCapability } from "../../lib/ipc";
import type { FetchNowState } from "../../hooks/useOriginFetch";
import type { I18n } from "@lingui/core";
import { useLingui } from "@lingui/react";

import { panelMessages as m } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";

interface Props {
  updateVersion: string | null;
  onOpenUpdate: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  scanning: boolean;
  updateCheckNotice: UpdateCheckNotice | null;
  upstream: UpstreamStatus | null;
  showRepoSync: boolean;
  fetchNow: FetchNowState;
  fetchCapability: RepoFetchCapability | null;
  onFetchNow: () => void;
  searchOpen: boolean;
  onToggleSearch: () => void;
  pinned: boolean;
  onTogglePinned: () => void;
  maximized: boolean;
  maximizeBusy: boolean;
  chromeDoubleClickMaximize: boolean;
  onToggleMaximized: () => void;
  onDismiss: () => void;
}

/** Stable top row for navigation, transient status and window actions.
 * Repository scope and view lenses live in the separate filter row below,
 * so neither group has to fight the other for horizontal space. */
export function PanelChromeRow({
  updateVersion,
  onOpenUpdate,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  scanning,
  updateCheckNotice,
  upstream,
  showRepoSync,
  fetchNow,
  fetchCapability,
  onFetchNow,
  searchOpen,
  onToggleSearch,
  pinned,
  onTogglePinned,
  maximized,
  maximizeBusy,
  chromeDoubleClickMaximize,
  onToggleMaximized,
  onDismiss,
}: Props) {
  const { i18n } = useLingui();
  return (
    <div className="panel-chrome-row">
      {updateVersion ? (
        <button
          type="button"
          className="panel-header-iconwrap panel-header-update-button has-update"
          title={translate(i18n, m.updateAvailableTitle, {
            version: updateVersion,
          })}
          aria-label={translate(i18n, m.updateAria, {
            version: updateVersion,
          })}
          onClick={onOpenUpdate}
          data-no-drag
        >
          <img
            src="/icon.png"
            alt=""
            className="panel-header-icon"
            draggable={false}
          />
          <span className="panel-header-update-badge" aria-hidden="true">
            !
          </span>
        </button>
      ) : (
        <span className="panel-header-iconwrap" title="gitwink">
          <img
            src="/icon.png"
            alt="gitwink"
            className="panel-header-icon"
            draggable={false}
          />
        </span>
      )}

      {(canGoBack || canGoForward) && (
        <span className="view-nav" role="group" aria-label={i18n._(m.viewHistory)}>
          <button
            type="button"
            className="view-nav-btn"
            onClick={onBack}
            disabled={!canGoBack}
            title={i18n._(m.backTitle)}
            aria-label={i18n._(m.back)}
          >
            ‹
          </button>
          <button
            type="button"
            className="view-nav-btn"
            onClick={onForward}
            disabled={!canGoForward}
            title={i18n._(m.forwardTitle)}
            aria-label={i18n._(m.forward)}
          >
            ›
          </button>
        </span>
      )}

      <div
        className="panel-drag-handle"
        onDoubleClick={
          chromeDoubleClickMaximize
            ? (event) => {
                event.preventDefault();
                event.stopPropagation();
                onToggleMaximized();
              }
            : undefined
        }
      />

      <div
        className={
          "panel-chrome-status" + (showRepoSync ? " has-repo-sync" : "")
        }
        aria-live="polite"
      >
        {scanning ? (
          <span className="panel-status">{i18n._(m.scanning)}</span>
        ) : updateCheckNotice ? (
          <span
            className={
              "panel-status" +
              (updateCheckNotice.kind === "error" ? " error" : "")
            }
            title={updateCheckNotice.detail}
          >
            {updateCheckNoticeLabel(updateCheckNotice, i18n)}
          </span>
        ) : null}
        {showRepoSync && upstream && <UpstreamBadge status={upstream} />}
        {showRepoSync && (
          <FetchNowButton
            state={fetchNow}
            fetchCapability={fetchCapability}
            onFetch={onFetchNow}
          />
        )}
      </div>

      <button
        type="button"
        className={"panel-search-btn" + (searchOpen ? " active" : "")}
        onClick={onToggleSearch}
        title={i18n._(m.searchCommitsTitle)}
        aria-label={searchOpen ? i18n._(m.closeSearch) : i18n._(m.searchCommits)}
        aria-pressed={searchOpen}
      >
        ⌕
      </button>
      <button
        type="button"
        className={"panel-pin" + (pinned ? " pinned" : "")}
        onClick={onTogglePinned}
        title={
          pinned
            ? i18n._(m.unpinTitle)
            : i18n._(m.pinTitle)
        }
        aria-label={pinned ? i18n._(m.unpinPanel) : i18n._(m.pinPanel)}
        aria-pressed={pinned}
      >
        📌
      </button>
      <button
        type="button"
        className={"panel-maximize" + (maximized ? " maximized" : "")}
        onClick={onToggleMaximized}
        disabled={maximizeBusy}
        title={maximized ? i18n._(m.restoreTitle) : i18n._(m.maximizeTitle)}
        aria-label={maximized ? i18n._(m.restorePanel) : i18n._(m.maximizePanel)}
        aria-pressed={maximized}
      >
        <span className="panel-maximize-glyph" aria-hidden="true" />
      </button>
      <button
        type="button"
        className="panel-close"
        onClick={onDismiss}
        title={i18n._(m.closeTitle)}
        aria-label={i18n._(m.closePanel)}
      >
        ✕
      </button>
    </div>
  );
}

export function updateCheckNoticeLabel(
  notice: UpdateCheckNotice,
  i18n?: I18n,
): string {
  switch (notice.kind) {
    case "checking":
      return i18n ? i18n._(m.updateChecking) : "Checking updates…";
    case "current":
      return i18n ? i18n._(m.updateCurrent) : "✓ Up to date";
    case "error":
      return i18n ? i18n._(m.updateFailed) : "⚠ Update check failed";
  }
}

function fetchNowLabel(i18n: I18n, state: FetchNowState): string {
  switch (state) {
    case "idle":
      return i18n._(m.fetchNow);
    case "fetching":
      return i18n._(m.fetchFetching);
    case "ok":
      return i18n._(m.fetchCurrent);
    case "in_progress":
    case "busy":
      return i18n._(m.fetchBusy);
    case "no_origin":
      return i18n._(m.fetchNoOrigin);
    case "authentication":
      return i18n._(m.fetchAuthentication);
    case "network":
      return i18n._(m.fetchNetwork);
    case "ref_lock":
      return i18n._(m.fetchRefLock);
    case "timeout":
      return i18n._(m.fetchTimeout);
    case "unavailable":
      return i18n._(m.fetchUnavailable);
    case "failed":
      return i18n._(m.fetchFailed);
  }
}

function FetchNowButton({
  state,
  fetchCapability,
  onFetch,
}: {
  state: FetchNowState;
  fetchCapability: RepoFetchCapability | null;
  onFetch: () => void;
}) {
  const { i18n } = useLingui();
  const effectiveState =
    fetchCapability === "no_origin"
      ? "no_origin"
      : fetchCapability === "unavailable"
        ? "unavailable"
        : state;
  const failed = [
    "authentication",
    "network",
    "ref_lock",
    "timeout",
    "unavailable",
    "failed",
  ].includes(effectiveState);
  const label = fetchNowLabel(i18n, effectiveState);
  const title =
    effectiveState === "fetching"
      ? i18n._(m.fetchFetchingTitle)
      : effectiveState === "ok"
        ? i18n._(m.fetchCurrentTitle)
        : effectiveState === "busy" || effectiveState === "in_progress"
          ? i18n._(m.fetchBusyTitle)
          : effectiveState === "idle"
            ? i18n._(m.fetchNowTitle)
            : label;
  const disabled =
    fetchCapability !== "available" ||
    effectiveState === "fetching" ||
    effectiveState === "in_progress" ||
    effectiveState === "no_origin";
  const glyph =
    effectiveState === "ok"
      ? "✓"
      : effectiveState === "no_origin"
        ? "–"
        : failed
          ? "!"
          : "↻";
  return (
    <button
      type="button"
      className={
        "fetch-now-btn" +
        (effectiveState === "fetching" ? " fetching" : "") +
        (failed ? " failed" : "")
      }
      disabled={disabled}
      onClick={onFetch}
      aria-label={label}
      title={title}
    >
      {glyph}
    </button>
  );
}

function formatFetchAge(i18n: I18n, unixSeconds: number): string {
  const ageSec = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (ageSec < 60) return i18n._(m.justNow);
  if (ageSec < 3600) {
    return translate(i18n, m.minutesAgo, { count: Math.floor(ageSec / 60) });
  }
  if (ageSec < 86_400) {
    return translate(i18n, m.hoursAgo, { count: Math.floor(ageSec / 3600) });
  }
  return translate(i18n, m.daysAgo, { count: Math.floor(ageSec / 86_400) });
}

/** Compact local-vs-upstream status. It shares the chrome status area with
 * scanning/update messages instead of consuming filter-row width. */
function UpstreamBadge({ status }: { status: UpstreamStatus }) {
  const { i18n } = useLingui();
  const synced = status.ahead === 0 && status.behind === 0;
  const ahead = status.ahead.toString() + (status.aheadCapped ? "+" : "");
  const behind = status.behind.toString() + (status.behindCapped ? "+" : "");
  const fetchHint = status.lastFetchUnix
    ? translate(i18n, m.lastFetch, {
        age: formatFetchAge(i18n, status.lastFetchUnix),
      })
    : i18n._(m.noFetch);
  const title = synced
    ? translate(i18n, m.upstreamSyncedTitle, {
        local: status.localBranch,
        upstream: status.upstream,
        fetch: fetchHint,
      })
    : translate(i18n, m.upstreamDivergedTitle, {
        local: status.localBranch,
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        fetch: fetchHint,
      });

  return (
    <span
      className={
        "upstream-badge" +
        (synced ? " upstream-badge-synced" : " upstream-badge-diverged")
      }
      title={title}
      aria-label={
        synced
          ? translate(i18n, m.upstreamSyncedAria, {
              upstream: status.upstream,
            })
          : translate(i18n, m.upstreamDivergedAria, {
              ahead: status.ahead,
              behind: status.behind,
              upstream: status.upstream,
            })
      }
    >
      {synced ? (
        <span className="upstream-badge-check" aria-hidden="true">
          ✓
        </span>
      ) : (
        <>
          {status.ahead > 0 && (
            <span className="upstream-badge-ahead">↑{ahead}</span>
          )}
          {status.behind > 0 && (
            <span className="upstream-badge-behind">↓{behind}</span>
          )}
        </>
      )}
    </span>
  );
}
