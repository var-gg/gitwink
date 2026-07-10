interface RepoCommitsReuseCoverage {
  incomplete: boolean;
  shallow: boolean;
}

export function reusableRepoCommitsFingerprint(
  fingerprint: string,
  coverage: RepoCommitsReuseCoverage,
): string | null {
  if (fingerprint === "" || coverage.incomplete || coverage.shallow) return null;
  return fingerprint;
}

export function canReuseRepoCommitsSnapshot(
  previousFingerprint: string | null,
  currentFingerprint: string,
): boolean {
  return (
    previousFingerprint != null &&
    currentFingerprint !== "" &&
    previousFingerprint === currentFingerprint
  );
}
