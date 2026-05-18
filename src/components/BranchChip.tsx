import { useEffect, useMemo, useState } from "react";

import type { BranchInfo } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  branches: BranchInfo[];
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

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return branches;
    return branches.filter((b) => b.name.toLowerCase().includes(q));
  }, [branches, query]);

  const label =
    selected === "all"
      ? `All branches (${branches.length})`
      : selected.length === 0
        ? "No branches"
        : selected.length === 1
          ? selected[0]
          : `${selected.length} branches`;

  function toggle(name: string) {
    if (selected === "all") {
      const others = branches.map((b) => b.name).filter((n) => n !== name);
      onChange(others);
      return;
    }
    const set = new Set(selected);
    if (set.has(name)) set.delete(name);
    else set.add(name);
    const next = Array.from(set);
    onChange(next.length === branches.length ? "all" : next);
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
        {filtered.map((b) => {
          const isSelected =
            selected === "all" || (selected as string[]).includes(b.name);
          return (
            <button
              key={b.name}
              type="button"
              className={"chip-item" + (isSelected ? " checked" : "")}
              onClick={() => toggle(b.name)}
            >
              <span className="chip-check">{isSelected ? "✓" : ""}</span>
              <span className="chip-item-name">
                {b.name}
                {b.isHead && <span className="chip-item-head"> · HEAD</span>}
              </span>
              <span className="chip-item-count">{b.commitCount}</span>
            </button>
          );
        })}
        {filtered.length === 0 && (
          <div className="chip-empty">No branches match.</div>
        )}
      </div>
    </ChipDropdown>
  );
}
