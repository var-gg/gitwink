import { useEffect, useState } from "react";
import type { I18n } from "@lingui/core";
import { useLingui } from "@lingui/react/macro";

import { filterMessages } from "../i18n/messages/filters";
import { translate } from "../i18n/translate";
import type { WindowDays } from "../types";
import { ChipDropdown } from "./ChipDropdown";
import { AuthorScopePage } from "./search-scope/AuthorScopePage";
import { RepositoryScopePage } from "./search-scope/RepositoryScopePage";
import { TimeScopePage } from "./search-scope/TimeScopePage";
import type {
  SearchScopeChipProps,
  SearchScopePage,
} from "./search-scope/types";

/** Responsive search-scope editor. Regular/wide panels show the three full
 * chips directly; compact/regular panels use this one disclosure so the
 * query keeps enough typing width without hiding any active restriction. */
export function SearchScopeChip(props: SearchScopeChipProps) {
  const { i18n } = useLingui();
  const [page, setPage] = useState<SearchScopePage>("summary");
  const [repoQuery, setRepoQuery] = useState("");
  const [authorQuery, setAuthorQuery] = useState("");

  useEffect(() => {
    if (!props.open) {
      setPage("summary");
      setRepoQuery("");
      setAuthorQuery("");
    }
  }, [props.open]);

  const narrowedCount =
    (props.selectedRepoPath != null || props.selectedRepoPaths !== "all" ? 1 : 0) +
    (props.selectedAuthors !== "all" ? 1 : 0) +
    (props.windowDays !== "all" ? 1 : 0);
  const label =
    narrowedCount === 0
      ? i18n._(filterMessages.searchScopeButton)
      : translate(i18n, filterMessages.searchScopeButtonCount, {
          count: narrowedCount,
        });

  return (
    <ChipDropdown
      id="search-scope"
      label={label}
      title={props.scopeLabel}
      open={props.open}
      onToggle={props.onToggle}
      onClose={props.onClose}
      align="right"
      active={narrowedCount > 0}
    >
      {page === "summary" ? (
        <ScopeSummary {...props} onOpenPage={setPage} />
      ) : page === "repositories" ? (
        <RepositoryScopePage
          {...props}
          query={repoQuery}
          onQueryChange={setRepoQuery}
          onBack={() => setPage("summary")}
        />
      ) : page === "authors" ? (
        <AuthorScopePage
          authors={props.authors}
          loading={props.authorsLoading}
          selected={props.selectedAuthors}
          query={authorQuery}
          onQueryChange={setAuthorQuery}
          onChange={props.onAuthorsChange}
          onBack={() => setPage("summary")}
        />
      ) : (
        <TimeScopePage
          value={props.windowDays}
          onChange={props.onWindowDaysChange}
          onBack={() => setPage("summary")}
          onClose={props.onClose}
        />
      )}
    </ChipDropdown>
  );
}

function ScopeSummary(
  props: SearchScopeChipProps & {
    onOpenPage: (page: SearchScopePage) => void;
  },
) {
  const { i18n } = useLingui();
  const selectedRepo = props.selectedRepoPath
    ? props.repos.find((repo) => repo.path === props.selectedRepoPath)
    : null;
  const repoLabel = selectedRepo
    ? selectedRepo.name
    : props.selectedRepoPaths === "all"
      ? translate(i18n, filterMessages.allReposCount, {
          count: props.repos.length,
        })
      : translate(i18n, filterMessages.repoCount, {
          count: props.selectedRepoPaths.length,
        });
  const authorLabel =
    props.selectedAuthors === "all"
      ? i18n._(filterMessages.allAuthors)
      : props.selectedAuthors.length === 0
        ? i18n._(filterMessages.noAuthors)
        : props.selectedAuthors.length === 1
          ? props.selectedAuthors[0]
          : translate(i18n, filterMessages.authorCount, {
              count: props.selectedAuthors.length,
            });

  return (
    <div className="search-scope-menu">
      <div className="search-scope-menu-title">
        {i18n._(filterMessages.searchScopeTitle)}
      </div>
      <ScopeSummaryRow
        label={i18n._(filterMessages.searchScopeRepositories)}
        value={repoLabel}
        onClick={() => props.onOpenPage("repositories")}
      />
      <ScopeSummaryRow
        label={i18n._(filterMessages.searchScopeAuthors)}
        value={authorLabel}
        onClick={() => props.onOpenPage("authors")}
      />
      <ScopeSummaryRow
        label={i18n._(filterMessages.searchScopeTime)}
        value={compactTimeLabel(i18n, props.windowDays)}
        onClick={() => props.onOpenPage("time")}
      />
      {props.singleMode && (
        <div className="search-scope-branch-note">
          {i18n._(filterMessages.searchAllBranchesNote)}
        </div>
      )}
    </div>
  );
}

function ScopeSummaryRow({
  label,
  value,
  onClick,
}: {
  label: string;
  value: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="search-scope-summary-row" onClick={onClick}>
      <span className="search-scope-summary-kind">{label}</span>
      <span className="search-scope-summary-value" title={value}>
        {value}
      </span>
      <span className="search-scope-summary-chevron" aria-hidden="true">
        ›
      </span>
    </button>
  );
}

function compactTimeLabel(i18n: I18n, value: WindowDays): string {
  if (value === "all") return i18n._(filterMessages.allTime);
  if (value === 1) return "24h";
  return `${value}d`;
}
