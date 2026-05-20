import { useEffect, useMemo, useState } from "react";

import type { Repo } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  repos: Repo[];
  pinned: string[];
  selectedPath: string | null;
  selectedPaths: string[] | "all";
  onSelect: (path: string | null) => void;
  onSelectMulti: (paths: string[] | "all") => void;
  onTogglePin: (path: string) => void;
  onHide: (path: string) => void;
  totalRepoCount: number;
}

export function RepoChip({
  open,
  onToggle,
  onClose,
  repos,
  pinned,
  selectedPath,
  selectedPaths,
  onSelect,
  onSelectMulti,
  onTogglePin,
  onHide,
  totalRepoCount,
}: Props) {
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

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
  const label = selected ? (
    <>
      {selected.name}
      <span
        className="chip-clear"
        onClick={(e) => {
          e.stopPropagation();
          onSelect(null);
        }}
        title="Back to All repos"
      >
        ✕
      </span>
    </>
  ) : selectedPaths === "all" ? (
    <>All repos ({totalRepoCount})</>
  ) : (
    <>
      {selectedPaths.length} repos
      <span
        className="chip-clear"
        onClick={(e) => {
          e.stopPropagation();
          onSelectMulti("all");
        }}
        title="Clear filter"
      >
        ✕
      </span>
    </>
  );

  return (
    <ChipDropdown
      id="repo"
      label={label}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
    >
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search repos…"
        />
      </div>
      <div className="chip-list">
        {pinnedRepos.length > 0 && (
          <>
            <div className="chip-section">📌 Pinned</div>
            {pinnedRepos.map((r) => (
              <RepoItem
                key={r.path}
                repo={r}
                pinned={livePinned.has(r.path)}
                active={selectedPath === r.path}
                selected={Array.isArray(selectedPaths) && selectedPaths.includes(r.path)}
                checkboxReadonly={singleRepoMode}
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
                onHide={() => onHide(r.path)}
              />
            ))}
          </>
        )}
        <button
          type="button"
          className={
            "chip-section-all" +
            (selectedPath == null && selectedPaths === "all" ? " active" : "")
          }
          onClick={() => {
            onSelect(null);
            onSelectMulti("all");
            onClose();
          }}
        >
          All repos
        </button>
        {otherRepos.map((r) => (
          <RepoItem
            key={r.path}
            repo={r}
            pinned={livePinned.has(r.path)}
            active={selectedPath === r.path}
            selected={Array.isArray(selectedPaths) && selectedPaths.includes(r.path)}
            checkboxReadonly={singleRepoMode}
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
            onHide={() => onHide(r.path)}
          />
        ))}
        {pinnedRepos.length === 0 && otherRepos.length === 0 && (
          <div className="chip-empty">No repos match.</div>
        )}
      </div>
    </ChipDropdown>
  );
}

function RepoItem({
  repo,
  pinned,
  active,
  selected,
  checkboxReadonly,
  onSelect,
  onToggleSelect,
  onPin,
  onHide,
}: {
  repo: Repo;
  pinned: boolean;
  active: boolean;
  selected: boolean;
  checkboxReadonly: boolean;
  onSelect: () => void;
  onToggleSelect: () => void;
  onPin: () => void;
  onHide: () => void;
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
            ? "Multi-select filter is available in all-repos mode"
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
        onClick={onSelect}
        title={
          isMissing
            ? `${repo.path} — moved or deleted on disk. Drop the new path on the panel to relink, or click ✕ to hide.`
            : repo.path
        }
      >
        <span className="chip-item-name">
          {repo.name}
          {isMissing && (
            <span className="chip-item-missing-tag"> · missing</span>
          )}
        </span>
        <span className="chip-item-path">{repo.path}</span>
      </button>
      {isMissing ? (
        <button
          type="button"
          className="chip-hide"
          onClick={(e) => {
            e.stopPropagation();
            onHide();
          }}
          title="Hide this repo (won't auto-rediscover)"
        >
          ✕
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
        >
          ★
        </button>
      )}
    </div>
  );
}
