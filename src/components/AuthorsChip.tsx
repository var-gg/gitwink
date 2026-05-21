import { useCallback, useEffect, useMemo, useState } from "react";

import type { AuthorTally } from "../types";
import { ChipDropdown } from "./ChipDropdown";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";

// Virtualised-row heights (px) — mirror the box heights in styles.css.
const ITEM_H = 26; // .chip-item — one-line author entry
const EMPTY_H = 34; // .chip-empty — "No authors match."

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

  const toggle = useCallback(
    (name: string) => {
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
    },
    [selected, authors, onChange],
  );

  // Flatten into virtual rows: the "All authors" reset button, then one
  // row per author, then the empty-state line when nothing matches.
  const rows = useMemo<VirtualChipRow[]>(() => {
    const out: VirtualChipRow[] = [];
    out.push({
      key: "__all",
      height: ITEM_H,
      render: () => (
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
      ),
    });
    for (const a of filtered) {
      const { name, count } = a;
      const isSelected =
        selected === "all" || (selected as string[]).includes(name);
      out.push({
        key: "author:" + name,
        height: ITEM_H,
        render: () => (
          <button
            type="button"
            className={"chip-item" + (isSelected ? " checked" : "")}
            onClick={() => toggle(name)}
          >
            <span className="chip-check">{isSelected ? "✓" : ""}</span>
            <span className="chip-item-name">{name}</span>
            <span className="chip-item-count">{count}</span>
          </button>
        ),
      });
    }
    if (filtered.length === 0) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => <div className="chip-empty">No authors match.</div>,
      });
    }
    return out;
  }, [filtered, selected, onChange, onClose, toggle]);

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
      <VirtualChipList rows={rows} resetKey={query} />
    </ChipDropdown>
  );
}
