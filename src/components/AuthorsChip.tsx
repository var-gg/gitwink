import { useCallback, useEffect, useMemo, useState } from "react";

import type { AuthorTally } from "../types";
import {
  removeUnavailableFacetValue,
  toggleFacetValue,
  unavailableFacetValues,
} from "../lib/facetSelection";
import { ChipDropdown } from "./ChipDropdown";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";
import { chipRowH, useUiScale } from "../lib/settings";

// Virtualised-row BASE heights (px) at scale 1.0 — chipRowH scales them
// against the current --ui-scale so JS row heights match the CSS content.
const ITEM_H_BASE = 26; // .chip-item — one-line author entry
const HEADER_H_BASE = 25; // .chip-section-header — unavailable selection label
const EMPTY_H_BASE = 34; // .chip-empty — "No authors match."

// Author list ordering. With "all repo / all branch" the author list can be
// large, so the order is user-controllable and persisted.
type AuthorSort = "count" | "name" | "recent";
const SORT_KEY = "gitwink.authorSort";
function loadAuthorSort(): AuthorSort {
  if (typeof window === "undefined") return "count";
  const v = window.localStorage.getItem(SORT_KEY);
  return v === "name" || v === "recent" ? v : "count";
}
const SORT_OPTIONS: { value: AuthorSort; label: string; title: string }[] = [
  { value: "count", label: "Count", title: "Most commits first" },
  { value: "name", label: "A–Z", title: "Alphabetical" },
  { value: "recent", label: "Recent", title: "Most recent activity first" },
];
const byName = (a: AuthorTally, b: AuthorTally) =>
  a.name.localeCompare(b.name, undefined, { sensitivity: "base" });

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
  const [sort, setSort] = useState<AuthorSort>(loadAuthorSort);
  const scale = useUiScale();

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  function changeSort(next: AuthorSort) {
    setSort(next);
    try {
      window.localStorage.setItem(SORT_KEY, next);
    } catch {}
  }

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? authors.filter((a) => a.name.toLowerCase().includes(q))
      : authors.slice();
    const cmp =
      sort === "name"
        ? byName
        : sort === "recent"
          ? (a: AuthorTally, b: AuthorTally) => b.lastActivity - a.lastActivity
          : (a: AuthorTally, b: AuthorTally) =>
              b.count - a.count || byName(a, b);
    return list.sort(cmp);
  }, [authors, query, sort]);

  const authorNames = useMemo(
    () => authors.map((author) => author.name),
    [authors],
  );
  const unavailableSelected = useMemo(
    () => unavailableFacetValues(authorNames, selected),
    [authorNames, selected],
  );
  const visibleUnavailable = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q
      ? unavailableSelected.filter((name) => name.toLowerCase().includes(q))
      : unavailableSelected;
  }, [query, unavailableSelected]);

  const label =
    selected === "all"
      ? "All authors"
      : selected.length === 0
        ? "No authors"
        : selected.length === 1
          ? unavailableSelected.length === 1
            ? `${selected[0]} · 0`
            : selected[0]
          : unavailableSelected.length > 0
            ? `${selected.length} authors · ${unavailableSelected.length} unavailable`
            : `${selected.length} authors`;

  const toggle = useCallback(
    (name: string) => {
      onChange(toggleFacetValue(selected, name, authorNames));
    },
    [selected, authorNames, onChange],
  );

  // Flatten into virtual rows: the "All authors" reset button, then one
  // row per author, then the empty-state line when nothing matches.
  const rows = useMemo<VirtualChipRow[]>(() => {
    const ITEM_H = chipRowH(scale, ITEM_H_BASE);
    const HEADER_H = chipRowH(scale, HEADER_H_BASE);
    const EMPTY_H = chipRowH(scale, EMPTY_H_BASE);

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
    if (visibleUnavailable.length > 0) {
      out.push({
        key: "__unavailable",
        height: HEADER_H,
        render: () => (
          <div
            className="chip-section-header chip-section-warning"
            title="Selected authors with no commits in the current repo, branch and time scope"
          >
            Selected · unavailable in this scope
          </div>
        ),
      });
      for (const name of visibleUnavailable) {
        out.push({
          key: "unavailable-author:" + name,
          height: ITEM_H,
          render: () => (
            <button
              type="button"
              className="chip-item checked chip-item-unavailable"
              title={`${name} has no commits in the current scope. Click to remove this filter.`}
              onClick={() => {
                onChange(removeUnavailableFacetValue(selected, name));
              }}
            >
              <span className="chip-check">✓</span>
              <span className="chip-item-name">{name}</span>
              <span className="chip-item-count">0</span>
            </button>
          ),
        });
      }
    }
    for (const a of visible) {
      const { name, count } = a;
      // In "all" the top row carries the highlight; individual rows aren't
      // pre-checked, so clicking one reads as "show only this author".
      const isSelected =
        selected !== "all" && (selected as string[]).includes(name);
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
            <span
              className="chip-only"
              role="button"
              tabIndex={-1}
              title={`Show only ${name}`}
              onClick={(e) => {
                e.stopPropagation();
                onChange([name]);
              }}
            >
              only
            </span>
            <span
              className="chip-item-count"
              title="Commits in the current repository and time scope"
            >
              {count}
            </span>
          </button>
        ),
      });
    }
    if (visible.length === 0 && visibleUnavailable.length === 0) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => <div className="chip-empty">No authors match.</div>,
      });
    }
    return out;
  }, [
    visible,
    visibleUnavailable,
    selected,
    onChange,
    onClose,
    toggle,
    scale,
  ]);

  return (
    <ChipDropdown
      id="authors"
      label={label}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
      align="right"
      active={selected !== "all"}
    >
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search authors…"
        />
      </div>
      <div className="chip-sort" role="group" aria-label="Sort authors">
        <span className="chip-sort-label">Sort</span>
        {SORT_OPTIONS.map((o) => (
          <button
            key={o.value}
            type="button"
            className={"chip-sort-btn" + (sort === o.value ? " active" : "")}
            title={o.title}
            onClick={() => changeSort(o.value)}
          >
            {o.label}
          </button>
        ))}
      </div>
      <VirtualChipList rows={rows} resetKey={query + ":" + sort} />
    </ChipDropdown>
  );
}
