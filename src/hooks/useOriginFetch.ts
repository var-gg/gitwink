import { useCallback, useEffect, useRef, useState } from "react";

import {
  fetchRepoNow,
  repoFetchCapability,
  type FetchRepoResult,
  type RepoFetchCapability,
} from "../lib/ipc";
import { sameCanonicalPath } from "../lib/repoCatalog";

export type FetchNowState = "idle" | "fetching" | FetchRepoResult;

interface OriginFetchOptions {
  repoPath: string | null;
  enabled: boolean;
  refreshNonce: number;
  onFetched: () => void;
}

/** Own the explicit origin-refresh lifecycle for the currently viewed repo. */
export function useOriginFetch({
  repoPath,
  enabled,
  refreshNonce,
  onFetched,
}: OriginFetchOptions): {
  state: FetchNowState;
  capability: RepoFetchCapability | null;
  runFetch: () => Promise<void>;
} {
  const [state, setState] = useState<FetchNowState>("idle");
  const [capability, setCapability] =
    useState<RepoFetchCapability | null>(null);
  const repoPathRef = useRef(repoPath);
  const resultTimerRef = useRef<number | null>(null);
  repoPathRef.current = repoPath;

  const clearResultTimer = useCallback(() => {
    if (resultTimerRef.current != null) {
      window.clearTimeout(resultTimerRef.current);
      resultTimerRef.current = null;
    }
  }, []);

  // Recheck on panel summon so adding or removing `origin` outside gitwink is
  // reflected without contacting the network.
  useEffect(() => {
    if (!enabled || !repoPath) {
      setCapability(null);
      return;
    }
    let cancelled = false;
    setCapability(null);
    repoFetchCapability(repoPath)
      .then((next) => {
        if (!cancelled) setCapability(next);
      })
      .catch(() => {
        if (!cancelled) setCapability("unavailable");
      });
    return () => {
      cancelled = true;
    };
  }, [enabled, refreshNonce, repoPath]);

  // A result belongs only to the repository that initiated it.
  useEffect(() => {
    clearResultTimer();
    setState("idle");
  }, [clearResultTimer, repoPath]);

  useEffect(() => clearResultTimer, [clearResultTimer]);

  const runFetch = useCallback(async () => {
    const requestedRepo = repoPathRef.current;
    if (!enabled || !requestedRepo) return;

    clearResultTimer();
    setState("fetching");
    let result: FetchRepoResult;
    try {
      result = await fetchRepoNow(requestedRepo);
    } catch {
      result = "failed";
    }

    const currentRepo = repoPathRef.current;
    if (
      currentRepo == null ||
      !sameCanonicalPath(currentRepo, requestedRepo)
    ) {
      return;
    }

    setState(result);
    if (result === "no_origin") setCapability("no_origin");
    if (result === "unavailable") setCapability("unavailable");
    if (result === "ok") onFetched();
    resultTimerRef.current = window.setTimeout(() => {
      resultTimerRef.current = null;
      setState((current) => (current === "fetching" ? current : "idle"));
    }, 2600);
  }, [clearResultTimer, enabled, onFetched]);

  return { state, capability, runFetch };
}
