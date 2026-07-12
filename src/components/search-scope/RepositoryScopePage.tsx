import { useMemo } from "react";
import { useLingui } from "@lingui/react/macro";

import { filterMessages } from "../../i18n/messages/filters";
import { chipRowH, useUiScale } from "../../lib/settings";
import { VirtualChipList, type VirtualChipRow } from "../VirtualChipList";
import { SearchScopePageHeader } from "./SearchScopePageHeader";
import type { SearchScopeChipProps } from "./types";

const REPO_ROW_H_BASE = 38;
const EMPTY_ROW_H_BASE = 36;

export function RepositoryScopePage(
  props: SearchScopeChipProps & {
    query: string;
    onQueryChange: (query: string) => void;
    onBack: () => void;
  },
) {
  const { i18n } = useLingui();
  const scale = useUiScale();
  const q = props.query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      props.repos.filter(
        (repo) =>
          !q ||
          repo.name.toLowerCase().includes(q) ||
          repo.path.toLowerCase().includes(q),
      ),
    [props.repos, q],
  );
  const rowHeight = chipRowH(scale, REPO_ROW_H_BASE);
  const emptyHeight = chipRowH(scale, EMPTY_ROW_H_BASE);
  const rows = useMemo<VirtualChipRow[]>(() => {
    if (visible.length === 0) {
      return [
        {
          key: "__empty",
          height: emptyHeight,
          render: () => (
            <div className="chip-empty">
              {i18n._(filterMessages.noReposMatch)}
            </div>
          ),
        },
      ];
    }
    return visible.map((repo) => {
      const multiSelected =
        Array.isArray(props.selectedRepoPaths) &&
        props.selectedRepoPaths.includes(repo.path);
      const singleSelected = props.selectedRepoPath === repo.path;
      const count = props.repoCounts.get(repo.id);
      return {
        key: `repo:${repo.path}`,
        height: rowHeight,
        render: () => (
          <div
            className={
              "search-scope-repo-row" + (singleSelected ? " active" : "")
            }
          >
            <button
              type="button"
              className="search-scope-repo-open"
              onClick={() => {
                props.onSelectRepo(repo.path);
                props.onClose();
              }}
              title={`${repo.name} — ${repo.path}`}
            >
              <span className="search-scope-repo-name">{repo.name}</span>
              <span className="search-scope-repo-path">{repo.path}</span>
            </button>
            {!props.singleMode && (
              <button
                type="button"
                className="search-scope-check"
                role="checkbox"
                aria-checked={multiSelected}
                aria-label={`${repo.name}: ${i18n._(filterMessages.addToMultiRepo)}`}
                onClick={() => {
                  const current = Array.isArray(props.selectedRepoPaths)
                    ? props.selectedRepoPaths
                    : [];
                  const next = current.includes(repo.path)
                    ? current.filter((path) => path !== repo.path)
                    : [...current, repo.path];
                  props.onSelectRepos(next.length === 0 ? "all" : next);
                }}
              >
                {multiSelected ? "✓" : ""}
              </button>
            )}
            <span className="search-scope-result-count" aria-hidden="true">
              {props.repoCountsLoading
                ? "…"
                : count?.toLocaleString(i18n.locale) ?? "0"}
            </span>
          </div>
        ),
      };
    });
  }, [
    visible,
    emptyHeight,
    i18n,
    i18n.locale,
    props.selectedRepoPaths,
    props.selectedRepoPath,
    props.singleMode,
    props.repoCounts,
    props.repoCountsLoading,
    props.onSelectRepo,
    props.onSelectRepos,
    props.onClose,
    rowHeight,
  ]);

  return (
    <div className="search-scope-page">
      <SearchScopePageHeader
        title={i18n._(filterMessages.searchScopeRepositories)}
        onBack={props.onBack}
      />
      <div className="chip-search">
        <input
          autoFocus
          value={props.query}
          onChange={(event) => props.onQueryChange(event.target.value)}
          placeholder={i18n._(filterMessages.searchRepos)}
          aria-label={i18n._(filterMessages.searchReposAria)}
        />
      </div>
      <button
        type="button"
        className={
          "chip-section-all" +
          (props.selectedRepoPath == null && props.selectedRepoPaths === "all"
            ? " active"
            : "")
        }
        onClick={() => {
          props.onSelectAllRepos();
          props.onClose();
        }}
      >
        {i18n._(filterMessages.allRepos)}
      </button>
      <VirtualChipList rows={rows} maxHeight={210} resetKey={props.query} />
    </div>
  );
}
