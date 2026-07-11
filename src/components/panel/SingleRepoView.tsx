import type { MutableRefObject } from "react";
import { useLingui } from "@lingui/react";

import { panelMessages as m } from "../../i18n/messages/panel";
import { translate } from "../../i18n/translate";
import type {
  BranchInfo,
  CommitSummary,
  FileHistoryResult,
  WindowDays,
} from "../../types";
import { Timeline } from "../Timeline";
import type { ExpansionControl } from "../TimelineWindowed";

interface RepoCoverage {
  truncated: boolean;
  incomplete: boolean;
  limit: number;
  shallow: boolean;
}

interface FileHistoryScope {
  repoPath: string;
  filePath: string;
}

interface SingleRepoViewProps {
  filteredCommits: CommitSummary[] | null;
  allCommits: CommitSummary[] | null;
  commitsError: boolean;
  repoCoverage: RepoCoverage | null;
  fileHistory: FileHistoryScope | null;
  fileHistoryCoverage: Omit<FileHistoryResult, "rows"> | null;
  fileHistoryError: boolean;
  branches: BranchInfo[];
  selectedRepoPath: string | null;
  selectedAuthors: string[] | "all";
  selectedBranches: string[] | "all";
  windowDays: WindowDays;
  anchor: { hash: string; nonce: number; expand?: boolean } | null;
  expansionControlRef: MutableRefObject<ExpansionControl | null>;
  onBackToAllRepos: () => void;
  onRetryFileHistory: () => void;
  onExitFileHistory: () => void;
  onShowAllTime: () => void;
  onClearAuthors: () => void;
  onAllBranches: () => void;
  onOpenSearch: () => void;
}

/** The single-repository body's state matrix. App owns fetching/navigation;
 * this component owns the mutually-exclusive loading/error/empty/coverage
 * presentation so those branches can be tested without the panel controller. */
export function SingleRepoView(props: SingleRepoViewProps) {
  const { i18n } = useLingui();
  const {
    filteredCommits,
    allCommits,
    commitsError,
    repoCoverage,
    fileHistory,
    fileHistoryCoverage,
    fileHistoryError,
  } = props;

  if (filteredCommits == null) {
    return (
      <p className="panel-empty">
        {i18n._(fileHistory ? m.loadingFileHistory : m.loadingCommits)}
      </p>
    );
  }

  if (commitsError) {
    return (
      <EmptyAction text={i18n._(m.repoUnavailable)}>
        <Action onClick={props.onBackToAllRepos}>{i18n._(m.backToAllRepos)}</Action>
      </EmptyAction>
    );
  }

  if (fileHistory && fileHistoryError) {
    return (
      <EmptyAction text={i18n._(m.fileHistoryUnavailable)}>
        <Action onClick={props.onRetryFileHistory}>{i18n._(m.retry)}</Action>
        <Action onClick={props.onExitFileHistory}>{i18n._(m.exitFileHistory)}</Action>
      </EmptyAction>
    );
  }

  if (filteredCommits.length === 0) {
    if (fileHistory) {
      return (
        <EmptyAction
          text={fileEmptyText(i18n, fileHistory.filePath, fileHistoryCoverage)}
          sub={i18n._(m.renamesNotFollowed)}
        >
          <Action onClick={props.onExitFileHistory}>{i18n._(m.exitFileHistory)}</Action>
        </EmptyAction>
      );
    }
    return (
      <EmptyAction
        text={filterEmptyText(
          i18n,
          repoCoverage,
          props.windowDays,
          props.selectedAuthors,
          props.selectedBranches,
        )}
      >
        {props.windowDays !== "all" && (
          <Action onClick={props.onShowAllTime}>{i18n._(m.showAllTime)}</Action>
        )}
        {props.selectedAuthors !== "all" && (
          <Action onClick={props.onClearAuthors}>{i18n._(m.clearAuthorFilter)}</Action>
        )}
        {props.selectedBranches !== "all" && (
          <Action onClick={props.onAllBranches}>{i18n._(m.allBranches)}</Action>
        )}
        <Action onClick={props.onOpenSearch} title={i18n._(m.searchCommitsTitle)}>
          {i18n._(m.searchCommits)}
        </Action>
      </EmptyAction>
    );
  }

  return (
    <>
      {!fileHistory && repoCoverage &&
        (repoCoverage.truncated || repoCoverage.incomplete || repoCoverage.shallow) && (
          <div className="history-coverage" role="status">
            {repoCoverageText(i18n, repoCoverage, props.selectedAuthors)}
          </div>
        )}
      {fileHistory && fileHistoryCoverage?.truncated && (
        <div className="filehist-coverage" role="status">
          {fileCoverageText(i18n, fileHistoryCoverage, filteredCommits.length)}{" "}
          {i18n._(m.renamesNotFollowed)}
        </div>
      )}
      <Timeline
        key={`single:${props.selectedRepoPath}`}
        commits={filteredCommits}
        allCommits={allCommits ?? undefined}
        branches={props.branches}
        resetKey={`${fileHistory ? `file:${fileHistory.filePath}` : ""}${JSON.stringify(props.selectedBranches)}|${props.windowDays}|${JSON.stringify(props.selectedAuthors)}`}
        anchor={props.anchor}
        linear={fileHistory != null}
        expansionControlRef={props.expansionControlRef}
      />
    </>
  );
}

function EmptyAction({
  text,
  sub,
  children,
}: {
  text: string;
  sub?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="panel-empty">
      <p className="panel-empty-line">{text}</p>
      {sub && <p className="panel-empty-sub">{sub}</p>}
      <p className="panel-empty-actions">{children}</p>
    </div>
  );
}

function Action({
  children,
  onClick,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title?: string;
}) {
  return (
    <button type="button" className="panel-empty-action" onClick={onClick} title={title}>
      {children}
    </button>
  );
}

type Lingui = ReturnType<typeof useLingui>["i18n"];

function fileEmptyText(
  i18n: Lingui,
  path: string,
  coverage: Omit<FileHistoryResult, "rows"> | null,
): string {
  const file = path.split("/").pop() ?? path;
  switch (coverage?.truncated) {
    case "scan_limit":
      return translate(i18n, m.fileEmptyScanLimit, {
        file,
        count: coverage.commitsScanned.toLocaleString(i18n.locale),
      });
    case "incomplete":
      return translate(i18n, m.fileEmptyIncomplete, { file });
    case "shallow_boundary":
      return translate(i18n, m.fileEmptyShallow, { file });
    default:
      return translate(i18n, m.fileEmpty, { file });
  }
}

function filterEmptyText(
  i18n: Lingui,
  coverage: RepoCoverage | null,
  windowDays: WindowDays,
  authors: string[] | "all",
  branches: string[] | "all",
): string {
  let text = coverage?.truncated
    ? translate(i18n, m.noMatchLimited, {
        limit: coverage.limit.toLocaleString(i18n.locale),
      })
    : coverage?.incomplete
      ? i18n._(m.noMatchIncomplete)
      : coverage?.shallow
        ? i18n._(m.noMatchShallow)
        : windowDays === 1
          ? i18n._(m.noCommits24h)
          : windowDays !== "all"
            ? translate(i18n, m.noCommitsDays, { days: windowDays })
            : i18n._(m.noCommits);
  if (authors !== "all") {
    text += translate(i18n, m.authorsFiltered, { count: authors.length });
  }
  if (branches !== "all") {
    text += translate(i18n, m.branchesFiltered, { count: branches.length });
  }
  return text;
}

function repoCoverageText(
  i18n: Lingui,
  coverage: RepoCoverage,
  authors: string[] | "all",
): string {
  if (coverage.truncated) {
    return translate(
      i18n,
      authors === "all" ? m.coverageLimited : m.coverageAuthorLimited,
      { limit: coverage.limit.toLocaleString(i18n.locale) },
    );
  }
  return i18n._(coverage.incomplete ? m.coverageIncomplete : m.coverageShallow);
}

function fileCoverageText(
  i18n: Lingui,
  coverage: Omit<FileHistoryResult, "rows">,
  matchCount: number,
): string {
  switch (coverage.truncated) {
    case "match_limit":
      return translate(i18n, m.fileCoverageMatches, {
        count: matchCount.toLocaleString(i18n.locale),
      });
    case "scan_limit":
      return translate(i18n, m.fileCoverageScanned, {
        count: coverage.commitsScanned.toLocaleString(i18n.locale),
      });
    case "shallow_boundary":
      return i18n._(m.fileCoverageShallow);
    default:
      return i18n._(m.fileCoverageIncomplete);
  }
}
