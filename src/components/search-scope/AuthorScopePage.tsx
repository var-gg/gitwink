import { useMemo } from "react";
import { useLingui } from "@lingui/react/macro";

import { filterMessages } from "../../i18n/messages/filters";
import {
  removeUnavailableFacetValue,
  toggleFacetValue,
  unavailableFacetValues,
} from "../../lib/facetSelection";
import { chipRowH, useUiScale } from "../../lib/settings";
import type { AuthorTally } from "../../types";
import { VirtualChipList, type VirtualChipRow } from "../VirtualChipList";
import { SearchScopePageHeader } from "./SearchScopePageHeader";

const AUTHOR_ROW_H_BASE = 28;
const EMPTY_ROW_H_BASE = 36;

export function AuthorScopePage({
  authors,
  loading,
  selected,
  query,
  onQueryChange,
  onChange,
  onBack,
}: {
  authors: AuthorTally[];
  loading: boolean;
  selected: string[] | "all";
  query: string;
  onQueryChange: (query: string) => void;
  onChange: (authors: string[] | "all") => void;
  onBack: () => void;
}) {
  const { i18n } = useLingui();
  const scale = useUiScale();
  const authorNames = useMemo(() => authors.map((author) => author.name), [authors]);
  const unavailable = useMemo(
    () => (loading ? [] : unavailableFacetValues(authorNames, selected)),
    [authorNames, selected, loading],
  );
  const q = query.trim().toLowerCase();
  const visibleUnavailable = unavailable.filter(
    (name) => !q || name.toLowerCase().includes(q),
  );
  const visible = useMemo(
    () =>
      authors
        .filter((author) => !q || author.name.toLowerCase().includes(q))
        .slice()
        .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
    [authors, q],
  );
  const rowHeight = chipRowH(scale, AUTHOR_ROW_H_BASE);
  const emptyHeight = chipRowH(scale, EMPTY_ROW_H_BASE);
  const rows = useMemo<VirtualChipRow[]>(() => {
    if (loading) {
      return [
        {
          key: "__loading",
          height: emptyHeight,
          render: () => (
            <div className="chip-empty" role="status">
              {i18n._(filterMessages.updatingAuthors)}
            </div>
          ),
        },
      ];
    }
    const out: VirtualChipRow[] = [];
    for (const name of visibleUnavailable) {
      out.push({
        key: `unavailable:${name}`,
        height: rowHeight,
        render: () => (
          <button
            type="button"
            className="chip-item checked chip-item-unavailable"
            role="checkbox"
            aria-checked={true}
            onClick={() => onChange(removeUnavailableFacetValue(selected, name))}
          >
            <span className="chip-check" aria-hidden="true">✓</span>
            <span className="chip-item-name" title={name}>{name}</span>
            <span className="chip-item-count">0</span>
          </button>
        ),
      });
    }
    for (const author of visible) {
      const checked = selected !== "all" && selected.includes(author.name);
      out.push({
        key: `author:${author.name}`,
        height: rowHeight,
        render: () => (
          <button
            type="button"
            className={"chip-item" + (checked ? " checked" : "")}
            role="checkbox"
            aria-checked={checked}
            onClick={() =>
              onChange(toggleFacetValue(selected, author.name, authorNames))
            }
          >
            <span className="chip-check" aria-hidden="true">
              {checked ? "✓" : ""}
            </span>
            <span className="chip-item-name" title={author.name}>
              {author.name}
            </span>
            <span className="chip-item-count">
              {author.count.toLocaleString(i18n.locale)}
            </span>
          </button>
        ),
      });
    }
    if (out.length === 0) {
      out.push({
        key: "__empty",
        height: emptyHeight,
        render: () => (
          <div className="chip-empty">
            {i18n._(filterMessages.noAuthorsMatch)}
          </div>
        ),
      });
    }
    return out;
  }, [
    loading,
    emptyHeight,
    i18n,
    i18n.locale,
    visibleUnavailable,
    rowHeight,
    selected,
    visible,
    authorNames,
    onChange,
  ]);

  return (
    <div className="search-scope-page">
      <SearchScopePageHeader
        title={i18n._(filterMessages.searchScopeAuthors)}
        onBack={onBack}
      />
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder={i18n._(filterMessages.searchAuthors)}
          aria-label={i18n._(filterMessages.searchAuthorsAria)}
        />
      </div>
      <button
        type="button"
        className={"chip-section-all" + (selected === "all" ? " active" : "")}
        role="checkbox"
        aria-checked={selected === "all"}
        onClick={() => onChange("all")}
      >
        {i18n._(filterMessages.allAuthors)}
      </button>
      <VirtualChipList rows={rows} maxHeight={210} resetKey={query} />
    </div>
  );
}
