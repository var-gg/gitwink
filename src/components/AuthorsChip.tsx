import { useEffect, useMemo, useState } from "react";

import type { AuthorTally } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  authors: AuthorTally[];
  selected: string[] | "all";
  onChange: (sel: string[] | "all") => void;
}

export function AuthorsChip({
  open,
  onToggle,
  onClose,
  authors,
  selected,
  onChange,
}: Props) {
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return authors;
    return authors.filter((a) => a.name.toLowerCase().includes(q));
  }, [authors, query]);

  const label =
    selected === "all"
      ? "All authors"
      : selected.length === 0
        ? "No authors"
        : selected.length === 1
          ? selected[0]
          : `${selected.length} authors`;

  function toggle(name: string) {
    if (selected === "all") {
      // Switching from "all" to specific selection.
      const others = authors.map((a) => a.name).filter((n) => n !== name);
      onChange(others);
      return;
    }
    const set = new Set(selected);
    if (set.has(name)) set.delete(name);
    else set.add(name);
    const next = Array.from(set);
    onChange(next.length === authors.length ? "all" : next);
  }

  return (
    <ChipDropdown
      id="authors"
      label={label}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
      align="right"
    >
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search authors…"
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
          <span className="chip-item-name">All authors</span>
        </button>
        {filtered.map((a) => {
          const isSelected =
            selected === "all" || (selected as string[]).includes(a.name);
          return (
            <button
              key={a.name}
              type="button"
              className={"chip-item" + (isSelected ? " checked" : "")}
              onClick={() => toggle(a.name)}
            >
              <span className="chip-check">{isSelected ? "✓" : ""}</span>
              <span className="chip-item-name">{a.name}</span>
              <span className="chip-item-count">{a.count}</span>
            </button>
          );
        })}
        {filtered.length === 0 && (
          <div className="chip-empty">No authors match.</div>
        )}
      </div>
    </ChipDropdown>
  );
}
