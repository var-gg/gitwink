import { useLingui } from "@lingui/react/macro";

import { panelMessages } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";
import { AuthorsChip } from "../AuthorsChip";
import { RepoChip } from "../RepoChip";
import { SearchBar } from "../SearchBar";
import { SearchScopeChip } from "../SearchScopeChip";
import { SEARCH_RESULTS_LIST_ID } from "../searchAccessibility";
import { TimeRangeChip } from "../TimeRangeChip";
import type { PanelFilterRowProps } from "./PanelFilterRow";

/** Search owns the query input, but not a parallel hidden filter universe.
 * This row keeps the shared repository/author/time scope editable while the
 * result list is active. */
export function PanelSearchFilterRow(props: PanelFilterRowProps) {
  const { i18n } = useLingui();
  const setChip = (chip: "repo" | "time" | "authors" | "searchScope") =>
    props.onOpenChip(props.openChip === chip ? null : chip);
  const inputScope =
    props.searchInput.trim() === ""
      ? translate(i18n, panelMessages.typeToSearch, {
          scope: props.searchScopeLabel,
        })
      : props.searchScopeLabel;

  return (
    <>
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
        scopeLabel={inputScope}
        scopeVisible={false}
        onChange={props.onSearchChange}
        onClose={props.onCloseSearch}
        onMove={(direction) => {
          if (props.searchResolved) {
            props.searchControlRef.current?.moveSelection(direction);
          }
        }}
        onActivate={props.onActivateSearch}
      />

      {props.compactSearchScope ? (
        <SearchScopeChip
          open={props.openChip === "searchScope"}
          onToggle={() => setChip("searchScope")}
          onClose={() => props.onOpenChip(null)}
          scopeLabel={props.searchScopeLabel}
          repos={props.repos}
          repoCounts={props.repoCounts}
          repoCountsLoading={props.repoCountsLoading}
          selectedRepoPath={props.selectedRepoPath}
          selectedRepoPaths={props.selectedRepoPaths}
          onSelectRepo={props.onSelectRepo}
          onSelectRepos={props.onSelectRepos}
          onSelectAllRepos={props.onSelectAllRepos}
          authors={props.authors}
          authorsLoading={props.authorsLoading}
          selectedAuthors={props.selectedAuthors}
          onAuthorsChange={props.onAuthorsChange}
          windowDays={props.windowDays}
          onWindowDaysChange={props.onWindowDaysChange}
          singleMode={props.singleMode}
        />
      ) : (
        <div className="search-scope-chips">
          <RepoChip
            open={props.openChip === "repo"}
            onToggle={() => setChip("repo")}
            onClose={() => props.onOpenChip(null)}
            repos={props.repos}
            repoCounts={props.repoCounts}
            loading={props.repoCountsLoading}
            countsInSingleMode
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
          <TimeRangeChip
            open={props.openChip === "time"}
            onToggle={() => setChip("time")}
            onClose={() => props.onOpenChip(null)}
            value={props.windowDays}
            onChange={props.onWindowDaysChange}
          />
          <AuthorsChip
            open={props.openChip === "authors"}
            onToggle={() => setChip("authors")}
            onClose={() => props.onOpenChip(null)}
            authors={props.authors}
            loading={props.authorsLoading}
            selected={props.selectedAuthors}
            onChange={props.onAuthorsChange}
          />
          {props.singleMode && (
            <span
              className="search-all-branches-badge"
              title={i18n._(panelMessages.allBranches)}
            >
              {i18n._(panelMessages.allBranches)}
            </span>
          )}
        </div>
      )}
    </>
  );
}
