import type { MutableRefObject } from "react";
import { useLingui } from "@lingui/react";

import { panelMessages as m } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";
import type {
  AuthorTally,
  BranchInfo,
  FileHistoryResult,
  HistoryIndexStatus,
  Repo,
  WindowDays,
} from "../../types";
import { AuthorsChip } from "../AuthorsChip";
import { BranchChip } from "../BranchChip";
import { RepoChip } from "../RepoChip";
import { SearchBar } from "../SearchBar";
import { SEARCH_RESULTS_LIST_ID } from "../searchAccessibility";
import { TimeRangeChip } from "../TimeRangeChip";
import type { SearchControl } from "../TimelineWindowed";

const EMPTY_REPO_COUNTS = new Map<number, number>();

export type OpenPanelChip = "repo" | "time" | "authors" | "branch" | null;

interface PanelFilterRowProps {
  searchOpen: boolean;
  searchInput: string;
  searching: boolean;
  searchResolved: boolean;
  searchCount: number | null;
  searchFocusNonce: number;
  searchActiveDescendant: string | null;
  searchScopeLabel: string | null;
  historyDemanded: boolean;
  historyIndexError: boolean;
  historyIndexStatus: HistoryIndexStatus | null;
  searchControlRef: MutableRefObject<SearchControl | null>;
  onSearchChange: (value: string) => void;
  onCloseSearch: () => void;
  onActivateSearch: () => void;
  openChip: OpenPanelChip;
  onOpenChip: (chip: OpenPanelChip) => void;
  repos: Repo[];
  repoCounts: Map<number, number>;
  repoCountsLoading: boolean;
  pinnedRepos: string[];
  selectedRepoPath: string | null;
  selectedRepoPaths: string[] | "all";
  onSelectRepo: (path: string | null) => void;
  onSelectRepos: (paths: string[] | "all") => void;
  onSelectAllRepos: () => void;
  onToggleRepoPin: (path: string) => void;
  onExcludeRepo: (path: string) => Promise<void>;
  fileHistory: { repoPath: string; filePath: string } | null;
  fileHistoryCoverage: Omit<FileHistoryResult, "rows"> | null;
  fileHistoryError: boolean;
  fileHistoryMatchCount: number;
  onExitFileHistory: () => void;
  singleMode: boolean;
  branches: BranchInfo[];
  branchesLoading: boolean;
  selectedBranches: string[] | "all";
  onBranchesChange: (selection: string[] | "all") => void;
  windowDays: WindowDays;
  onWindowDaysChange: (days: WindowDays) => void;
  authors: AuthorTally[];
  authorsLoading: boolean;
  selectedAuthors: string[] | "all";
  onAuthorsChange: (authors: string[] | "all") => void;
}

/** The second header row: repository scope plus view lenses, or the search
 * command surface. Keeping it separate from PanelChromeRow is the deliberate
 * two-row layout contract; adding a new lens cannot crowd window controls. */
export function PanelFilterRow(props: PanelFilterRowProps) {
  const { i18n } = useLingui();
  const setChip = (chip: Exclude<OpenPanelChip, null>) =>
    props.onOpenChip(props.openChip === chip ? null : chip);

  return (
    <div
      className={
        "panel-filter-row" + (props.searchOpen ? " panel-filter-row-search" : "")
      }
      data-no-drag
    >
      {props.searchOpen ? (
        <SearchBar
          value={props.searchInput}
          count={props.searching && props.searchResolved ? props.searchCount : null}
          focusNonce={props.searchFocusNonce}
          resultsListId={SEARCH_RESULTS_LIST_ID}
          resultsAvailable={
            props.searching &&
            props.searchResolved &&
            props.searchCount != null &&
            props.searchCount > 0
          }
          resultsBusy={props.searching && !props.searchResolved}
          activeDescendant={
            props.searchResolved ? props.searchActiveDescendant : null
          }
          scopeLabel={searchInputScope(i18n, props)}
          onChange={props.onSearchChange}
          onClose={props.onCloseSearch}
          onMove={(direction) => {
            if (props.searchResolved) {
              props.searchControlRef.current?.moveSelection(direction);
            }
          }}
          onActivate={props.onActivateSearch}
        />
      ) : (
        <div className="header-chips">
          <RepoChip
            open={props.openChip === "repo"}
            onToggle={() => setChip("repo")}
            onClose={() => props.onOpenChip(null)}
            repos={props.repos}
            repoCounts={props.singleMode ? EMPTY_REPO_COUNTS : props.repoCounts}
            loading={!props.singleMode && props.repoCountsLoading}
            pinned={props.pinnedRepos}
            selectedPath={props.selectedRepoPath}
            selectedPaths={props.selectedRepoPaths}
            onSelect={props.onSelectRepo}
            onSelectMulti={props.onSelectRepos}
            onSelectAll={props.onSelectAllRepos}
            onTogglePin={props.onToggleRepoPin}
            onExclude={props.onExcludeRepo}
            totalRepoCount={props.repos.length}
          />

          {props.fileHistory ? (
            <FileHistoryChip {...props} />
          ) : (
            <>
              {props.singleMode && (
                <span className="chip-slot chip-slot-primary">
                  <BranchChip
                    open={props.openChip === "branch"}
                    onToggle={() => setChip("branch")}
                    onClose={() => props.onOpenChip(null)}
                    repoPath={props.selectedRepoPath ?? ""}
                    branches={props.branches}
                    loading={props.branchesLoading}
                    selected={props.selectedBranches}
                    onChange={props.onBranchesChange}
                  />
                </span>
              )}
              <span className="chip-slot chip-slot-time">
                <TimeRangeChip
                  open={props.openChip === "time"}
                  onToggle={() => setChip("time")}
                  onClose={() => props.onOpenChip(null)}
                  value={props.windowDays}
                  onChange={props.onWindowDaysChange}
                />
              </span>
              <span className="chip-slot chip-slot-authors">
                <AuthorsChip
                  open={props.openChip === "authors"}
                  onToggle={() => setChip("authors")}
                  onClose={() => props.onOpenChip(null)}
                  authors={props.authors}
                  loading={!props.singleMode && props.authorsLoading}
                  selected={props.selectedAuthors}
                  onChange={props.onAuthorsChange}
                />
              </span>
            </>
          )}
        </div>
      )}
    </div>
  );
}

type Lingui = ReturnType<typeof useLingui>["i18n"];

function searchInputScope(i18n: Lingui, props: PanelFilterRowProps): string {
  const scope = props.searchScopeLabel ?? i18n._(m.allReposScope);
  if (props.searchInput.trim() === "") {
    return translate(i18n, m.typeToSearch, { scope });
  }
  return translate(i18n, m.searchScope, {
    scope,
    history: completeHistoryScope(i18n, props),
    branches: props.singleMode ? i18n._(m.allBranchesSuffix) : "",
  });
}

function completeHistoryScope(
  i18n: Lingui,
  props: Pick<
    PanelFilterRowProps,
    "historyDemanded" | "historyIndexError" | "historyIndexStatus"
  >,
): string {
  if (!props.historyDemanded) return i18n._(m.historyAll);
  if (props.historyIndexError) return i18n._(m.historyIndexed);
  if (!props.historyIndexStatus) return i18n._(m.historyPreparingShort);
  if (props.historyIndexStatus.allComplete) return i18n._(m.historyAll);
  return translate(
    i18n,
    props.historyIndexStatus.settled
      ? m.historyIndexedCount
      : m.historyIndexingCount,
    {
      published: props.historyIndexStatus.publishedRepos,
      total: props.historyIndexStatus.totalRepos,
    },
  );
}

function FileHistoryChip(props: PanelFilterRowProps) {
  const { i18n } = useLingui();
  const coverage = fileHistoryScope(i18n, props);
  return (
    <button
      type="button"
      className="filehist-chip"
      onClick={props.onExitFileHistory}
      title={translate(i18n, m.fileHistoryTitle, {
        path: props.fileHistory?.filePath ?? "",
        scope: coverage.title,
      })}
    >
      <span className="filehist-chip-icon" aria-hidden="true">🕘</span>
      <span className="filehist-chip-name">
        {props.fileHistory?.filePath.split("/").pop()}
      </span>
      <span className="filehist-chip-scope">{coverage.label}</span>
      <span className="filehist-chip-x" aria-hidden="true">✕</span>
    </button>
  );
}

function fileHistoryScope(i18n: Lingui, props: PanelFilterRowProps) {
  const value = props.fileHistoryCoverage;
  const label = props.fileHistoryError
    ? i18n._(m.fileHistoryUnavailableShort)
    : value == null
      ? i18n._(m.scanningShort)
      : value.truncated === "match_limit"
        ? translate(i18n, m.latestChanges, {
            count: props.fileHistoryMatchCount.toLocaleString(i18n.locale),
          })
        : value.truncated === "scan_limit"
          ? translate(i18n, m.commitsScannedShort, {
              count: value.commitsScanned.toLocaleString(i18n.locale),
            })
          : value.truncated === "incomplete"
            ? i18n._(m.partialHistory)
            : value.truncated === "shallow_boundary"
              ? i18n._(m.cloneHistory)
              : i18n._(m.historyAll);
  return {
    label,
    title: value
      ? translate(i18n, m.fileHistoryRefs, {
          scope: label,
          count: value.remoteRefsIncluded.toLocaleString(i18n.locale),
        })
      : label,
  };
}
