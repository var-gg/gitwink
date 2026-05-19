import { useEffect, useMemo, useState } from "react";

import type { BranchInfo } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  branches: BranchInfo[];
  /** Array of refNames (e.g. "refs/heads/main", "refs/remotes/origin/main")
   * or the "all" sentinel. We key by refName so a local "main" and a
   * remote "origin/main" never collide. */
  selected: string[] | "all";
  onChange: (sel: string[] | "all") => void;
}

export function BranchChip({
  open,
  onToggle,
  onClose,
  branches,
  selected,
  onChange,
}: Props) {
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  const { localBranches, remoteBranches } = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = (b: BranchInfo) =>
      !q || b.name.toLowerCase().includes(q);
    return {
      localBranches: branches.filter((b) => b.kind === "local" && match(b)),
      remoteBranches: branches.filter((b) => b.kind === "remote" && match(b)),
    };
  }, [branches, query]);

  // Label adapts to the count. For a single selection we show the branch
  // name itself — minimum cognitive load — and fall back to a count when
  // multiple are picked.
  const label = useMemo(() => {
    if (selected === "all") return `All branches (${branches.length})`;
    if (selected.length === 0) return "No branches";
    if (selected.length === 1) {
      const only = branches.find((b) => b.refName === selected[0]);
      return only?.name ?? "1 branch";
    }
    return `${selected.length} branches`;
  }, [selected, branches]);

  function toggle(refName: string) {
    if (selected === "all") {
      // First explicit click: every branch is currently "on", so toggling
      // one means "everything except this one".
      const others = branches
        .map((b) => b.refName)
        .filter((r) => r !== refName);
      onChange(others);
      return;
    }
    const set = new Set(selected);
    if (set.has(refName)) set.delete(refName);
    else set.add(refName);
    const next = Array.from(set);
    onChange(next.length === branches.length ? "all" : next);
  }

  function renderItem(b: BranchInfo) {
    const isSelected =
      selected === "all" || (selected as string[]).includes(b.refName);
    return (
      <button
        key={b.refName}
        type="button"
        className={"chip-item" + (isSelected ? " checked" : "")}
        onClick={() => toggle(b.refName)}
      >
        <span className="chip-check">{isSelected ? "✓" : ""}</span>
        <span className="chip-item-name">
          {b.name}
          {b.isHead && <span className="chip-item-head"> · HEAD</span>}
        </span>
        <span className="chip-item-count">{b.commitCount}</span>
      </button>
    );
  }

  return (
    <ChipDropdown
      id="branch"
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
          placeholder="Search branches…"
        />
      </div>
      <div className="chip-list">
        <button
          type="button"
          className={"chip-item" + (selected === "all" ? " active" : "")}
          onClick={() => {
            onChange("all");
            onClose();
          }}
        >
          <span className="chip-item-name">All branches</span>
        </button>

        {localBranches.length > 0 && (
          <>
            <div className="chip-section-header">Local</div>
            {localBranches.map(renderItem)}
          </>
        )}

        {remoteBranches.length > 0 && (
          <>
            <div
              className="chip-section-header"
              title="Remote-tracking refs are local — gitwink never calls git fetch. Updated by your IDE / CLI."
            >
              Remote tracking
            </div>
            {remoteBranches.map(renderItem)}
          </>
        )}

        {localBranches.length === 0 && remoteBranches.length === 0 && (
          <div className="chip-empty">No branches match.</div>
        )}
      </div>
    </ChipDropdown>
  );
}
