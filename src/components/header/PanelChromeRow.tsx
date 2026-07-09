import type { UpstreamStatus } from "../../types";

export type FetchNowState = "idle" | "fetching" | "ok" | "failed" | "busy";

interface Props {
  updateVersion: string | null;
  onOpenUpdate: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  scanning: boolean;
  upToDate: boolean;
  upstream: UpstreamStatus | null;
  showRepoSync: boolean;
  fetchNow: FetchNowState;
  onFetchNow: () => void;
  searchOpen: boolean;
  onToggleSearch: () => void;
  pinned: boolean;
  onTogglePinned: () => void;
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
  upToDate,
  upstream,
  showRepoSync,
  fetchNow,
  onFetchNow,
  searchOpen,
  onToggleSearch,
  pinned,
  onTogglePinned,
  onDismiss,
}: Props) {
  return (
    <div className="panel-chrome-row">
      <div
        className={
          "panel-header-iconwrap" + (updateVersion ? " has-update" : "")
        }
        title={
          updateVersion
            ? `Update available — v${updateVersion} · click to update`
            : "gitwink"
        }
        role={updateVersion ? "button" : undefined}
        onClick={updateVersion ? onOpenUpdate : undefined}
        {...(updateVersion ? { "data-no-drag": true } : {})}
      >
        <img
          src="/icon.png"
          alt="gitwink"
          className="panel-header-icon"
          draggable={false}
        />
        {updateVersion && (
          <span className="panel-header-update-badge" aria-hidden="true">
            !
          </span>
        )}
      </div>

      {(canGoBack || canGoForward) && (
        <span className="view-nav" role="group" aria-label="View history">
          <button
            type="button"
            className="view-nav-btn"
            onClick={onBack}
            disabled={!canGoBack}
            title="Back (Alt+←)"
            aria-label="Back"
          >
            ‹
          </button>
          <button
            type="button"
            className="view-nav-btn"
            onClick={onForward}
            disabled={!canGoForward}
            title="Forward (Alt+→)"
            aria-label="Forward"
          >
            ›
          </button>
        </span>
      )}

      <div className="panel-drag-handle" />

      <div className="panel-chrome-status" aria-live="polite">
        {scanning ? (
          <span className="panel-status">Scanning…</span>
        ) : upToDate ? (
          <span className="panel-status">✓ Up to date</span>
        ) : null}
        {showRepoSync && upstream && <UpstreamBadge status={upstream} />}
        {showRepoSync && (
          <FetchNowButton state={fetchNow} onFetch={onFetchNow} />
        )}
      </div>

      <button
        type="button"
        className={"panel-search-btn" + (searchOpen ? " active" : "")}
        onClick={onToggleSearch}
        title="Search commits — message, author, SHA (/)"
        aria-label={searchOpen ? "Close commit search" : "Search commits"}
      >
        ⌕
      </button>
      <button
        type="button"
        className={"panel-pin" + (pinned ? " pinned" : "")}
        onClick={onTogglePinned}
        title={
          pinned
            ? "Unpin — return to glance mode (auto-hides on blur, always-on-top)"
            : "Pin — keep open while clicking elsewhere; not always-on-top. Summon via tray / hotkey."
        }
        aria-label={pinned ? "Unpin panel" : "Pin panel"}
      >
        📌
      </button>
      <button
        type="button"
        className="panel-close"
        onClick={onDismiss}
        title="Close (Esc) — closes diff window too"
        aria-label="Close panel"
      >
        ✕
      </button>
    </div>
  );
}

function FetchNowButton({
  state,
  onFetch,
}: {
  state: FetchNowState;
  onFetch: () => void;
}) {
  const failed = state === "failed" || state === "busy";
  const label =
    state === "fetching"
      ? "Fetching origin"
      : state === "ok"
        ? "Fetched — origin refs are current"
        : state === "busy"
          ? "Another fetch is running"
          : state === "failed"
            ? "Fetch failed"
            : "Fetch origin now";
  const title =
    state === "fetching"
      ? "Fetching origin…"
      : state === "ok"
        ? "Fetched ✓ — origin refs are current"
        : state === "busy"
          ? "Another fetch is running — try again in a moment"
          : state === "failed"
            ? "Fetch failed (offline? no origin? another fetch mid-flight?) — counts reflect the last successful fetch"
            : "Fetch origin now — refresh against the remote (read-only; never merges or pushes)";
  return (
    <button
      type="button"
      className={
        "fetch-now-btn" +
        (state === "fetching" ? " fetching" : "") +
        (failed ? " failed" : "")
      }
      disabled={state === "fetching"}
      onClick={onFetch}
      aria-label={label}
      title={title}
    >
      {state === "ok" ? "✓" : failed ? "!" : "↻"}
    </button>
  );
}

function formatFetchAge(unixSeconds: number): string {
  const ageSec = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (ageSec < 60) return "just now";
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86_400) return `${Math.floor(ageSec / 3600)}h ago`;
  return `${Math.floor(ageSec / 86_400)}d ago`;
}

/** Compact local-vs-upstream status. It shares the chrome status area with
 * scanning/update messages instead of consuming filter-row width. */
function UpstreamBadge({ status }: { status: UpstreamStatus }) {
  const synced = status.ahead === 0 && status.behind === 0;
  const ahead = status.ahead.toString() + (status.aheadCapped ? "+" : "");
  const behind = status.behind.toString() + (status.behindCapped ? "+" : "");
  const fetchHint = status.lastFetchUnix
    ? `Last fetch: ${formatFetchAge(status.lastFetchUnix)}`
    : "No fetch recorded yet";
  const title = synced
    ? `${status.localBranch} is in sync with ${status.upstream}.\n${fetchHint}. Reflects your last fetch.`
    : `${status.localBranch} vs ${status.upstream}: ${status.ahead} ahead, ${status.behind} behind.\n${fetchHint}. Reflects your last fetch.`;

  return (
    <span
      className={
        "upstream-badge" +
        (synced ? " upstream-badge-synced" : " upstream-badge-diverged")
      }
      title={title}
      aria-label={
        synced
          ? `In sync with ${status.upstream}`
          : `${status.ahead} ahead, ${status.behind} behind ${status.upstream}`
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
