import type { I18n } from "@lingui/core";

import { filterMessages } from "../../i18n/messages/filters";
import { panelMessages } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";
import type { HistoryIndexStatus, Repo, WindowDays } from "../../types";

export function repositorySearchScopeLabel(
  i18n: I18n,
  repoIds: number[] | null,
  repos: Repo[],
): string {
  if (repoIds == null) return i18n._(panelMessages.allReposScope);
  if (repoIds.length === 0) {
    return i18n._(panelMessages.selectedRepoScopeLoading);
  }
  if (repoIds.length === 1) {
    const id = repoIds[0];
    return (
      repos.find((repo) => repo.id === id)?.name ??
      translate(i18n, filterMessages.repoCount, { count: 1 })
    );
  }
  return translate(i18n, filterMessages.repoCount, {
    count: repoIds.length,
  });
}

export function authorSearchScopeLabel(
  i18n: I18n,
  selectedAuthors: string[] | "all",
): string {
  if (selectedAuthors === "all") return i18n._(filterMessages.allAuthors);
  if (selectedAuthors.length === 0) return i18n._(filterMessages.noAuthors);
  if (selectedAuthors.length === 1) return selectedAuthors[0];
  return translate(i18n, filterMessages.authorCount, {
    count: selectedAuthors.length,
  });
}

export function historySearchScopeLabel(
  i18n: I18n,
  windowDays: WindowDays,
  historyDemanded: boolean,
  historyIndexError: boolean,
  historyIndexStatus: HistoryIndexStatus | null,
): string {
  if (windowDays !== "all") {
    return windowDays === 1
      ? i18n._(filterMessages.last24Hours)
      : translate(i18n, filterMessages.lastDays, { count: windowDays });
  }
  if (!historyDemanded) return i18n._(panelMessages.historyAll);
  if (historyIndexError) return i18n._(panelMessages.historyIndexed);
  if (!historyIndexStatus) return i18n._(panelMessages.historyPreparingShort);
  if (historyIndexStatus.allComplete) return i18n._(panelMessages.historyAll);
  return translate(
    i18n,
    historyIndexStatus.settled
      ? panelMessages.historyIndexedCount
      : panelMessages.historyIndexingCount,
    {
      published: historyIndexStatus.publishedRepos,
      total: historyIndexStatus.totalRepos,
    },
  );
}

export function combinedSearchScopeLabel(
  i18n: I18n,
  repositories: string,
  history: string,
  authors: string,
  singleMode: boolean,
): string {
  return translate(i18n, panelMessages.searchScope, {
    scope: repositories,
    history,
    authors,
    branches: singleMode ? i18n._(panelMessages.allBranchesSuffix) : "",
  });
}
