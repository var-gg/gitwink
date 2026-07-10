import { useCallback, useEffect, useMemo, useState } from "react";

import type { Repo } from "../types";
import { ChipDropdown } from "./ChipDropdown";
import { ContextMenu } from "./ContextMenu";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";
import { chipRowH, useUiScale } from "../lib/settings";

// Virtualised-row BASE heights (px) at scale 1.0 — chipRowH multiplies
// these by the current --ui-scale so the JS row heights track the CSS
// content (which scales via calc(... * var(--ui-scale))).
const REPO_ROW_H_BASE = 40; // .chip-item-row — two-line repo entry
const SECTION_H_BASE = 21; // .chip-section — "📌 Pinned" label
const ALL_H_BASE = 23; // .chip-section-all — "All repos" reset button
const EMPTY_H_BASE = 34; // .chip-empty — "No repos match."

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  repos: Repo[];
  /** repo id → commit count in the active time window (RepoChip facet). */
  repoCounts: Map<number, number>;
  /** True while counts for a new cross-filter scope are being computed. The
   * previous scope's badges and checkbox mutations must not look current. */
  loading?: boolean;
  pinned: string[];
  selectedPath: string | null;
  selectedPaths: string[] | "all";
  onSelect: (path: string | null) => void;
  onSelectMulti: (paths: string[] | "all") => void;
  /** "All repos" resets BOTH scope dimensions as one navigation (one
   *  view-history entry) — not an onSelect + onSelectMulti pair. */
  onSelectAll: () => void;
  onTogglePin: (path: string) => void;
  onExclude: (path: string) => Promise<void>;
  totalRepoCount: number;
}

export function RepoChip({
  open,
  onToggle,
  onClose,
  repos,
  repoCounts,
  loading = false,
  pinned,
  selectedPath,
  selectedPaths,
  onSelect,
  onSelectMulti,
  onSelectAll,
  onTogglePin,
  onExclude,
  totalRepoCount,
}: Props) {
  const [query, setQuery] = useState("");
  const [contextTarget, setContextTarget] = useState<{
    path: string;
    x: number;
    y: number;
    restoreFocusTo: HTMLElement | null;
  } | null>(null);
  const [excludingPath, setExcludingPath] = useState<string | null>(null);
  const [excludeError, setExcludeError] = useState<string | null>(null);
  const scale = useUiScale();

  useEffect(() => {
    if (!open) {
      setQuery("");
      setContextTarget(null);
      setExcludeError(null);
    }
  }, [open]);

  const requestExclude = useCallback(
    async (path: string) => {
      if (excludingPath) return;
      setExcludingPath(path);
      setExcludeError(null);
      try {
        await onExclude(path);
      } catch (error) {
        setExcludeError(
          error instanceof Error ? error.message : String(error),
        );
      } finally {
        setExcludingPath(null);
      }
    },
    [excludingPath, onExclude],
  );

  // Snapshot of `pinned` taken when the dropdown opens. Section
  // membership (Pinned vs All) is decided against this snapshot, so
  // toggling a ★ while open never moves a repo between sections under
  // the cursor. The star glyph still reflects the live pin state for
  // immediate feedback; reopening re-snapshots and the reorder lands.
  const [pinnedSnapshot, setPinnedSnapshot] = useState<string[]>(pinned);
  useEffect(() => {
    if (open) setPinnedSnapshot(pinned);
    // `pinned` is intentionally omitted: the snapshot must NOT update
    // while the dropdown stays open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Snapshot of `selectedPaths` taken when the dropdown opens. The
  // selected-to-top sort keys off this snapshot, not the live value, so
  // ticking a checkbox never makes a row jump under the cursor — reopening
  // re-snapshots and the just-checked repos float up then. Same pattern as
  // `pinnedSnapshot` above and BranchChip's `snapshot`.
  const [selectedSnapshot, setSelectedSnapshot] = useState<string[] | "all">(
    selectedPaths,
  );
  useEffect(() => {
    if (open) setSelectedSnapshot(selectedPaths);
    // `selectedPaths` intentionally omitted — must NOT re-snapshot while open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const livePinned = useMemo(() => new Set(pinned), [pinned]);

  const repoByPath = useMemo(() => {
    const m = new Map<string, Repo>();
    for (const r of repos) m.set(r.path, r);
    return m;
  }, [repos]);

  const q = query.trim().toLowerCase();
  const matches = (r: Repo) =>
    !q ||
    r.name.toLowerCase().includes(q) ||
    r.path.toLowerCase().includes(q);

  // Single-repo mode: App ignores the multi-select filter, so the row
  // checkboxes render read-only instead of silently doing nothing.
  const singleRepoMode = selectedPath != null;
  const countsLoading = loading && !singleRepoMode;

  const selectedSnapSet = useMemo(
    () => (Array.isArray(selectedSnapshot) ? new Set(selectedSnapshot) : null),
    [selectedSnapshot],
  );

  // Section ordering: snapshot-selected repos float to the top of their
  // section. Sort is stable, so within each group the Pinned section keeps
  // pin order and the All section stays alphabetical.
  const pinnedRepos = useMemo(() => {
    const list = pinnedSnapshot
      .map((p) => repoByPath.get(p))
      .filter((r): r is Repo => !!r && matches(r));
    return list.sort(
      (a, b) =>
        (selectedSnapSet?.has(a.path) ? 0 : 1) -
        (selectedSnapSet?.has(b.path) ? 0 : 1),
    );
  }, [pinnedSnapshot, repoByPath, q, selectedSnapSet]);

  const otherRepos = useMemo(() => {
    const snapSet = new Set(pinnedSnapshot);
    const list = repos.filter((r) => !snapSet.has(r.path) && matches(r));
    return list.sort(
      (a, b) =>
        (selectedSnapSet?.has(a.path) ? 0 : 1) -
          (selectedSnapSet?.has(b.path) ? 0 : 1) ||
        a.name.localeCompare(b.name),
    );
  }, [repos, pinnedSnapshot, q, selectedSnapSet]);

  const selected = selectedPath ? repoByPath.get(selectedPath) : null;
  const label = selected
    ? selected.name
    : selectedPaths === "all"
      ? `All repos (${totalRepoCount})`
      : `${selectedPaths.length} ${selectedPaths.length === 1 ? "repo" : "repos"}`;
  const labelTitle = selected
    ? `${selected.name} — ${selected.path}`
    : label;
  const clearSelection = selected
    ? onSelectAll
    : selectedPaths === "all"
      ? undefined
      : () => onSelectMulti("all");
  const clearTitle = selected ? "Back to All repos" : "Clear repository filter";

  // Flatten the two sections into one virtual-row list. The Pinned header,
  // the "All repos" button and the empty-state line are known-height
  // special rows interleaved with the repo rows.
  const rows = useMemo<VirtualChipRow[]>(() => {
    // Scaled heights — shadow the module BASE values so the closure
    // below uses one integer set everywhere, recomputed when scale changes.
    const REPO_ROW_H = chipRowH(scale, REPO_ROW_H_BASE);
    const SECTION_H = chipRowH(scale, SECTION_H_BASE);
    const ALL_H = chipRowH(scale, ALL_H_BASE);
    const EMPTY_H = chipRowH(scale, EMPTY_H_BASE);

    const repoRow = (r: Repo): VirtualChipRow => ({
      key: "repo:" + r.path,
      height: REPO_ROW_H,
      render: () => (
        <RepoItem
          repo={r}
          count={repoCounts.get(r.id)}
          countLoading={countsLoading}
          pinned={livePinned.has(r.path)}
          active={selectedPath === r.path}
          selected={
            Array.isArray(selectedPaths) && selectedPaths.includes(r.path)
          }
          checkboxReadonly={singleRepoMode}
          checkboxReadonlyTitle="Multi-select filter is available in all-repos mode"
          onSelect={() => {
            onSelect(r.path);
            onClose();
          }}
          onToggleSelect={() => {
            const current = Array.isArray(selectedPaths) ? selectedPaths : [];
            const next = current.includes(r.path)
              ? current.filter((p) => p !== r.path)
              : [...current, r.path];
            onSelectMulti(next.length === 0 ? "all" : next);
          }}
          onPin={() => onTogglePin(r.path)}
          excluding={excludingPath === r.path}
          onExclude={() => void requestExclude(r.path)}
          onRowContextMenu={(x, y, restoreFocusTo) =>
            setContextTarget({ path: r.path, x, y, restoreFocusTo })
          }
        />
      ),
    });

    const out: VirtualChipRow[] = [];
    if (pinnedRepos.length > 0) {
      out.push({
        key: "__pinned",
        height: SECTION_H,
        render: () => <div className="chip-section">📌 Pinned</div>,
      });
      for (const r of pinnedRepos) out.push(repoRow(r));
    }
    out.push({
      key: "__all",
      height: ALL_H,
      render: () => (
        <button
          type="button"
          className={
            "chip-section-all" +
            (selectedPath == null && selectedPaths === "all" ? " active" : "")
          }
          aria-pressed={selectedPath == null && selectedPaths === "all"}
          onClick={() => {
            onSelectAll();
            onClose();
          }}
        >
          All repos
        </button>
      ),
    });
    for (const r of otherRepos) out.push(repoRow(r));
    if (pinnedRepos.length === 0 && otherRepos.length === 0) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => <div className="chip-empty">No repos match.</div>,
      });
    }
    return out;
  }, [
    pinnedRepos,
    otherRepos,
    repoCounts,
    livePinned,
    selectedPath,
    selectedPaths,
    singleRepoMode,
    countsLoading,
    onSelect,
    onSelectMulti,
    onSelectAll,
    onTogglePin,
    excludingPath,
    requestExclude,
    onClose,
    scale,
  ]);

  return (
    <>
      <ChipDropdown
        id="repo"
        label={label}
        title={labelTitle}
        open={open}
        onToggle={onToggle}
        onClose={onClose}
        active={selectedPath != null || selectedPaths !== "all"}
        onClear={clearSelection}
        clearTitle={clearTitle}
      >
        <div className="chip-search">
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search repos…"
            aria-label="Search repositories"
          />
        </div>
        {excludeError && (
          <div className="chip-action-error" role="alert">
            Couldn't exclude repository: {excludeError}
          </div>
        )}
        <VirtualChipList rows={rows} resetKey={query} />
      </ChipDropdown>
      {contextTarget && (
        <ContextMenu
          x={contextTarget.x}
          y={contextTarget.y}
          items={[
            {
              label: "Exclude from gitwink",
              description:
                "Keeps the folder and preferences. Restore later in Settings.",
              disabled: excludingPath != null,
              onClick: () => requestExclude(contextTarget.path),
            },
          ]}
          restoreFocusTo={contextTarget.restoreFocusTo}
          onClose={() => setContextTarget(null)}
        />
      )}
    </>
  );
}

function RepoItem({
  repo,
  count,
  countLoading,
  pinned,
  active,
  selected,
  checkboxReadonly,
  checkboxReadonlyTitle,
  onSelect,
  onToggleSelect,
  onPin,
  excluding,
  onExclude,
  onRowContextMenu,
}: {
  repo: Repo;
  /** commit count in the active window, or undefined when the repo has
   * none / its id isn't resolved yet. */
  count?: number;
  countLoading: boolean;
  pinned: boolean;
  active: boolean;
  selected: boolean;
  checkboxReadonly: boolean;
  checkboxReadonlyTitle: string;
  onSelect: () => void;
  onToggleSelect: () => void;
  onPin: () => void;
  excluding: boolean;
  onExclude: () => void;
  onRowContextMenu: (
    x: number,
    y: number,
    restoreFocusTo: HTMLElement | null,
  ) => void;
}) {
  const isMissing = repo.status === "missing";
  return (
    <div
      className={
        "chip-item-row" +
        (active ? " active" : "") +
        (isMissing ? " missing" : "") +
        (selected ? " selected" : "")
      }
      onContextMenu={(e) => {
        e.preventDefault();
        onRowContextMenu(
          e.clientX,
          e.clientY,
          e.currentTarget.querySelector<HTMLButtonElement>(".chip-item"),
        );
      }}
    >
      <button
        type="button"
        role="checkbox"
        aria-checked={selected}
        aria-disabled={checkboxReadonly || undefined}
        aria-label={`Filter timeline to ${repo.name}`}
        tabIndex={checkboxReadonly ? -1 : 0}
        className={
          "chip-checkbox" +
          (selected ? " checked" : "") +
          (checkboxReadonly ? " readonly" : "")
        }
        title={
          checkboxReadonly
            ? checkboxReadonlyTitle
            : "Add to multi-repo filter"
        }
        onClick={(e) => {
          e.stopPropagation();
          if (checkboxReadonly) return;
          onToggleSelect();
        }}
      >
        {selected && (
          <span className="chip-checkbox-icon" aria-hidden="true">
            ✓
          </span>
        )}
      </button>
      <button
        type="button"
        className="chip-item"
        // A missing repo can't be opened — selecting it would only produce
        // an error view, so the row is informational (tooltip explains how
        // to relink) and ✕ excludes it.
        onClick={() => {
          if (!isMissing) onSelect();
        }}
        onKeyDown={(e) => {
          if ((e.shiftKey && e.key === "F10") || e.key === "ContextMenu") {
            e.preventDefault();
            const rect = e.currentTarget.getBoundingClientRect();
            onRowContextMenu(
              rect.left + Math.min(16, rect.width / 2),
              rect.bottom,
              e.currentTarget,
            );
          }
        }}
        aria-disabled={isMissing || undefined}
        aria-label={
          isMissing
            ? `${repo.name}, repository missing at ${repo.path}`
            : `Open ${repo.name} at ${repo.path}`
        }
        title={
          isMissing
            ? `${repo.path} — moved or deleted on disk. Drop the new path on the panel to relink, or click ✕ to exclude it.`
            : repo.path
        }
      >
        <span className="chip-item-titleline">
          <span className="chip-item-name" title={repo.name}>
            {repo.name}
            {isMissing && (
              <span className="chip-item-missing-tag"> · missing</span>
            )}
          </span>
          {(countLoading || count != null) && (
            <span
              className={
                "chip-item-count" + (countLoading ? " loading" : "")
              }
              title={
                countLoading
                  ? "Updating commits for the current author and time filters"
                  : "Commits matching the current author and time filters"
              }
              aria-label={countLoading ? "Updating commit count" : undefined}
            >
              {countLoading ? "…" : count}
            </span>
          )}
        </span>
        <span className="chip-item-path">{repo.path}</span>
      </button>
      {isMissing ? (
        <button
          type="button"
          className="chip-hide"
          disabled={excluding}
          onClick={(e) => {
            e.stopPropagation();
            onExclude();
          }}
          title="Exclude from gitwink (restore later in Settings)"
          aria-label={`Exclude ${repo.name} from gitwink`}
        >
          {excluding ? "…" : "✕"}
        </button>
      ) : (
        <button
          type="button"
          className={"chip-pin" + (pinned ? " pinned" : "")}
          onClick={(e) => {
            e.stopPropagation();
            onPin();
          }}
          title={pinned ? "Unpin" : "Pin"}
          aria-label={pinned ? `Unpin ${repo.name}` : `Pin ${repo.name}`}
          aria-pressed={pinned}
        >
          ★
        </button>
      )}
    </div>
  );
}
