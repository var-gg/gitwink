import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLingui } from "@lingui/react/macro";

import type { AuthorTally } from "../types";
import {
  removeUnavailableFacetValue,
  toggleFacetValue,
  unavailableFacetValues,
} from "../lib/facetSelection";
import { ChipDropdown } from "./ChipDropdown";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";
import { chipRowH, useUiScale } from "../lib/settings";
import { filterMessages } from "../i18n/messages/filters";
import { translate } from "../i18n/translate";

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
const byName = (a: AuthorTally, b: AuthorTally) =>
  a.name.localeCompare(b.name, undefined, { sensitivity: "base" });

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  authors: AuthorTally[];
  /** While a new scope's facets are resolving, replace the old scope's rows
   * instead of allowing a stale author to be selected. */
  loading?: boolean;
  selected: string[] | "all";
  onChange: (sel: string[] | "all") => void;
}

export function AuthorsChip({
  open,
  onToggle,
  onClose,
  authors,
  loading = false,
  selected,
  onChange,
}: Props) {
  const { i18n } = useLingui();
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<AuthorSort>(loadAuthorSort);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const scale = useUiScale();
  const sortOptions: { value: AuthorSort; label: string; title: string }[] = [
    {
      value: "count",
      label: i18n._(filterMessages.sortCount),
      title: i18n._(filterMessages.sortCountTitle),
    },
    {
      value: "name",
      label: i18n._(filterMessages.sortName),
      title: i18n._(filterMessages.sortNameTitle),
    },
    {
      value: "recent",
      label: i18n._(filterMessages.sortRecent),
      title: i18n._(filterMessages.sortRecentTitle),
    },
  ];

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  useEffect(() => {
    if (!open || loading) return;
    window.requestAnimationFrame(() => searchRef.current?.focus());
  }, [open, loading]);

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
    () => (loading ? [] : unavailableFacetValues(authorNames, selected)),
    [authorNames, selected, loading],
  );
  const visibleUnavailable = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q
      ? unavailableSelected.filter((name) => name.toLowerCase().includes(q))
      : unavailableSelected;
  }, [query, unavailableSelected]);

  const label =
    selected === "all"
      ? i18n._(filterMessages.allAuthors)
      : selected.length === 0
        ? i18n._(filterMessages.noAuthors)
        : selected.length === 1
          ? unavailableSelected.length === 1
            ? `${selected[0]} · 0`
            : selected[0]
          : unavailableSelected.length > 0
            ? translate(i18n, filterMessages.authorCountUnavailable, {
                count: selected.length,
                unavailable: unavailableSelected.length,
              })
            : translate(i18n, filterMessages.authorCount, {
                count: selected.length,
              });

  const toggle = useCallback(
    (name: string) => {
      if (loading) return;
      onChange(toggleFacetValue(selected, name, authorNames));
    },
    [selected, authorNames, onChange, loading],
  );

  // Flatten into virtual rows: the "All authors" reset button, then one
  // row per author, then the empty-state line when nothing matches.
  const rows = useMemo<VirtualChipRow[]>(() => {
    const ITEM_H = chipRowH(scale, ITEM_H_BASE);
    const HEADER_H = chipRowH(scale, HEADER_H_BASE);
    const EMPTY_H = chipRowH(scale, EMPTY_H_BASE);

    const out: VirtualChipRow[] = [];
    if (loading) {
      out.push({
        key: "__loading",
        height: EMPTY_H,
        render: () => (
          <div className="chip-empty" role="status">
            {i18n._(filterMessages.updatingAuthors)}
          </div>
        ),
      });
      return out;
    }
    out.push({
      key: "__all",
      height: ITEM_H,
      render: () => (
        <button
          type="button"
          className={"chip-item" + (selected === "all" ? " active" : "")}
          role="checkbox"
          aria-checked={selected === "all"}
          onClick={() => {
            onChange("all");
            onClose();
          }}
        >
          <span className="chip-item-name">
            {i18n._(filterMessages.allAuthors)}
          </span>
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
            title={i18n._(filterMessages.unavailableAuthorsTitle)}
          >
            {i18n._(filterMessages.unavailableAuthors)}
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
              role="checkbox"
              aria-checked={true}
              title={translate(i18n, filterMessages.unavailableAuthorRemove, {
                author: name,
              })}
              onClick={() => {
                onChange(removeUnavailableFacetValue(selected, name));
              }}
            >
              <span className="chip-check" aria-hidden="true">
                ✓
              </span>
              <span className="chip-item-name" title={name}>
                {name}
              </span>
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
          <div
            className={"chip-item" + (isSelected ? " checked" : "")}
          >
            <button
              type="button"
              className="chip-author-toggle"
              role="checkbox"
              aria-checked={isSelected}
              title={name}
              onClick={() => toggle(name)}
            >
              <span className="chip-check" aria-hidden="true">
                {isSelected ? "✓" : ""}
              </span>
              <span className="chip-item-name" title={name}>
                {name}
              </span>
            </button>
            <button
              type="button"
              className="chip-only"
              title={translate(i18n, filterMessages.showOnlyAuthor, {
                author: name,
              })}
              aria-label={translate(i18n, filterMessages.showOnlyAuthor, {
                author: name,
              })}
              onClick={() => onChange([name])}
            >
              {i18n._(filterMessages.only)}
            </button>
            <span
              className="chip-item-count"
              title={i18n._(filterMessages.authorCountTitle)}
            >
              {count}
            </span>
          </div>
        ),
      });
    }
    if (visible.length === 0 && visibleUnavailable.length === 0) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => (
          <div className="chip-empty">
            {i18n._(filterMessages.noAuthorsMatch)}
          </div>
        ),
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
    loading,
    i18n,
    i18n.locale,
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
          ref={searchRef}
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={i18n._(filterMessages.searchAuthors)}
          aria-label={i18n._(filterMessages.searchAuthorsAria)}
          disabled={loading}
        />
      </div>
      <div
        className="chip-sort"
        role="group"
        aria-label={i18n._(filterMessages.sortAuthors)}
      >
        <span className="chip-sort-label">{i18n._(filterMessages.sort)}</span>
        {sortOptions.map((o) => (
          <button
            key={o.value}
            type="button"
            className={"chip-sort-btn" + (sort === o.value ? " active" : "")}
            title={o.title}
            aria-pressed={sort === o.value}
            disabled={loading}
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
