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
  onSelect: (path: string | null) => void;
  onTogglePin: (path: string) => void;
  totalRepoCount: number;
}

export function RepoChip({
  open,
  onToggle,
  onClose,
  repos,
  pinned,
  selectedPath,
  onSelect,
  onTogglePin,
  totalRepoCount,
}: Props) {
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

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

  const pinnedRepos = useMemo(
    () =>
      pinned
        .map((p) => repoByPath.get(p))
        .filter((r): r is Repo => !!r)
        .filter(matches),
    [pinned, repoByPath, q],
  );

  const otherRepos = useMemo(() => {
    const pinnedSet = new Set(pinned);
    return repos
      .filter((r) => !pinnedSet.has(r.path))
      .filter(matches)
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [repos, pinned, q]);

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
  ) : (
    <>All repos ({totalRepoCount})</>
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
        <button
          type="button"
          className={"chip-item" + (selectedPath == null ? " active" : "")}
          onClick={() => {
            onSelect(null);
            onClose();
          }}
        >
          <span className="chip-item-name">All repos</span>
        </button>
        {pinnedRepos.length > 0 && (
          <>
            <div className="chip-section">📌 Pinned</div>
            {pinnedRepos.map((r) => (
              <RepoItem
                key={r.path}
                repo={r}
                pinned
                active={selectedPath === r.path}
                onSelect={() => {
                  onSelect(r.path);
                  onClose();
                }}
                onPin={() => onTogglePin(r.path)}
              />
            ))}
          </>
        )}
        {otherRepos.length > 0 && (
          <>
            <div className="chip-section">All repos</div>
            {otherRepos.map((r) => (
              <RepoItem
                key={r.path}
                repo={r}
                pinned={false}
                active={selectedPath === r.path}
                onSelect={() => {
                  onSelect(r.path);
                  onClose();
                }}
                onPin={() => onTogglePin(r.path)}
              />
            ))}
          </>
        )}
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
  onSelect,
  onPin,
}: {
  repo: Repo;
  pinned: boolean;
  active: boolean;
  onSelect: () => void;
  onPin: () => void;
}) {
  return (
    <div className={"chip-item-row" + (active ? " active" : "")}>
      <button type="button" className="chip-item" onClick={onSelect}>
        <span className="chip-item-name">{repo.name}</span>
        <span className="chip-item-path">{repo.path}</span>
      </button>
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
    </div>
  );
}
