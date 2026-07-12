import type { CommitSummary } from "../types";
import { sameCanonicalPath } from "./repoCatalog";

/** Ensure an externally resolved commit remains renderable even when it is
 * older than the normal single-repo fetch cap. The direct resolver is the
 * authority for the one injected row; the normal timeline remains the
 * authority for every surrounding row. */
export function mergeAnchoredCommit(
  commits: CommitSummary[] | null,
  target: CommitSummary | null,
  selectedRepoPath: string | null,
  fileHistoryActive: boolean,
): CommitSummary[] | null {
  if (
    target == null ||
    selectedRepoPath == null ||
    fileHistoryActive ||
    !sameCanonicalPath(target.repoPath, selectedRepoPath)
  ) {
    return commits;
  }

  if (commits == null) return [target];
  if (commits.some((commit) => commit.hash === target.hash)) return commits;

  return [...commits, target].sort(
    (left, right) =>
      right.timestamp - left.timestamp || left.hash.localeCompare(right.hash),
  );
}

/** An anchored row can move when the bounded timeline arrives after the
 * directly resolved target. Follow that asynchronous re-indexing only while
 * the landing still owns the viewport; explicit user navigation retires the
 * tracking nonce. */
export function shouldApplyAnchoredRow(
  anchorNonce: number,
  appliedNonce: number,
  trackingNonce: number,
  targetIndex: number,
  appliedIndex: number,
): boolean {
  return (
    anchorNonce !== appliedNonce ||
    (trackingNonce === anchorNonce &&
      targetIndex >= 0 &&
      targetIndex !== appliedIndex)
  );
}

/** Restore the requested open/closed presentation while the landing still
 * owns navigation. This covers effect replay during a cold React mount;
 * direct user navigation first retires `trackingNonce`, so it is never
 * overridden. */
export function shouldRestoreAnchorExpansion(
  anchorNonce: number,
  trackingNonce: number,
  anchorHash: string,
  expand: boolean,
  expandedHash: string | null,
): boolean {
  const desiredHash = expand ? anchorHash : null;
  return trackingNonce === anchorNonce && expandedHash !== desiredHash;
}
