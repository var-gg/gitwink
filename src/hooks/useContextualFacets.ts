import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { listFilterFacets } from "../lib/ipc";
import { timelineSnapshotFilterKey } from "../lib/useTimelineWindow";
import type { AuthorTally, WindowDays } from "../types";

interface ContextualFacetsOptions {
  singleMode: boolean;
  repoIds: number[] | null;
  selectedAuthors: string[] | "all";
  windowDays: WindowDays;
}

/** Contextual author and repository-count facets for the fleet timeline.
 * Each facet excludes its own dimension, so changing authors does not freeze
 * the author picker and changing repository scope does not stale repo counts.
 * The visible timeline generation is the publication fence. */
export function useContextualFacets({
  singleMode,
  repoIds,
  selectedAuthors,
  windowDays,
}: ContextualFacetsOptions) {
  const [authors, setAuthors] = useState<AuthorTally[]>([]);
  const [repoCounts, setRepoCounts] = useState<Map<number, number>>(
    () => new Map(),
  );
  const [authorsLoading, setAuthorsLoading] = useState(true);
  const [repoCountsLoading, setRepoCountsLoading] = useState(true);
  const [error, setError] = useState(false);
  const [retryNonce, setRetryNonce] = useState(0);
  const [snapshot, setSnapshot] = useState<{
    key: string;
    generation: number;
    since: number | null;
  } | null>(null);
  const appliedAuthorsScope = useRef<string | null>(null);
  const appliedRepoScope = useRef<string | null>(null);

  const window = windowDays === "all" ? null : windowDays;
  const filterKey = useMemo(
    () =>
      timelineSnapshotFilterKey(
        repoIds,
        selectedAuthors === "all" ? null : selectedAuthors,
        window,
      ),
    [repoIds, selectedAuthors, window],
  );
  const authorsScopeKey = useMemo(
    () => JSON.stringify([repoIds, window]),
    [repoIds, window],
  );
  const repoScopeKey = useMemo(
    () => JSON.stringify([selectedAuthors, window]),
    [selectedAuthors, window],
  );

  const acceptSnapshot = useCallback(
    (generation: number, since: number | null, key: string) => {
      setSnapshot({ key, generation, since });
    },
    [],
  );
  const retry = useCallback(() => setRetryNonce((nonce) => nonce + 1), []);

  useEffect(() => {
    if (singleMode) {
      setAuthorsLoading(false);
      setRepoCountsLoading(false);
      setError(false);
      return;
    }
    const authorsApplyKey = `${authorsScopeKey}|${snapshot?.generation ?? "pending"}`;
    const repoApplyKey = `${repoScopeKey}|${snapshot?.generation ?? "pending"}`;
    const authorsStale = appliedAuthorsScope.current !== authorsApplyKey;
    const reposStale = appliedRepoScope.current !== repoApplyKey;

    if (!snapshot || snapshot.key !== filterKey) {
      setAuthorsLoading(authorsStale);
      setRepoCountsLoading(reposStale);
      setError(false);
      return;
    }
    if (!authorsStale && !reposStale) {
      setAuthorsLoading(false);
      setRepoCountsLoading(false);
      setError(false);
      return;
    }

    let cancelled = false;
    setAuthorsLoading(authorsStale);
    setRepoCountsLoading(reposStale);
    setError(false);
    void (async () => {
      try {
        const facets = await listFilterFacets(
          {
            since: snapshot.since,
            repoIds,
            authors: selectedAuthors === "all" ? null : selectedAuthors,
            viewGeneration: snapshot.generation,
          },
          authorsStale,
          reposStale,
        );
        if (cancelled) return;
        if (authorsStale) {
          setAuthors(facets.authors);
          appliedAuthorsScope.current = authorsApplyKey;
        }
        if (reposStale) {
          setRepoCounts(new Map(facets.repos.map((repo) => [repo.repoId, repo.count])));
          appliedRepoScope.current = repoApplyKey;
        }
        setError(false);
      } catch {
        if (cancelled) return;
        if (authorsStale) setAuthors([]);
        if (reposStale) setRepoCounts(new Map());
        setError(true);
      } finally {
        if (!cancelled) {
          setAuthorsLoading(false);
          setRepoCountsLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    singleMode,
    filterKey,
    snapshot,
    repoIds,
    selectedAuthors,
    retryNonce,
    authorsScopeKey,
    repoScopeKey,
  ]);

  return {
    authors,
    repoCounts,
    authorsLoading,
    repoCountsLoading,
    error,
    retry,
    acceptSnapshot,
  };
}
