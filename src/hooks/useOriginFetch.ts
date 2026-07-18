import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import {
  fetchRepoNow,
  onFetchState,
  repoFetchCapability,
  type FetchCommandResult,
  type FetchRepoResult,
  type RepoFetchCapability,
} from "../lib/ipc";
import { sameCanonicalPath } from "../lib/repoCatalog";
import {
  initialOriginFetchState,
  originFetchReducer,
  type FetchNowState,
} from "./originFetchState";

export type { FetchNowState } from "./originFetchState";

interface OriginFetchOptions {
  repoPath: string | null;
  enabled: boolean;
  capabilityNonce: number;
  onFetched: () => void;
}

interface CapabilityEntry {
  repoPath: string;
  value: RepoFetchCapability | null;
}

const capabilityCache = new Map<string, RepoFetchCapability>();

export function cachedOriginCapability(
  repoPath: string,
): RepoFetchCapability | undefined {
  const exact = capabilityCache.get(repoPath);
  if (exact) return exact;
  const windowsPath = /^[a-z]:[\\/]/i.test(repoPath);
  for (const [cachedPath, capability] of capabilityCache) {
    if (sameCanonicalPath(cachedPath, repoPath, windowsPath)) return capability;
  }
  return undefined;
}

export function rememberOriginCapability(
  repoPath: string,
  capability: RepoFetchCapability,
) {
  capabilityCache.set(repoPath, capability);
}

export function clearOriginCapabilityCacheForTest() {
  capabilityCache.clear();
}

export function acceptTerminalFlight(seen: Set<string>, flightId: string): boolean {
  if (seen.has(flightId)) return false;
  seen.add(flightId);
  if (seen.size > 64) {
    const oldest = seen.values().next().value as string | undefined;
    if (oldest) seen.delete(oldest);
  }
  return true;
}

/** Own the explicit and automatic origin-refresh lifecycle for the currently
 * viewed repository. Backend flight events are authoritative; command results
 * are a fallback and cannot regress a flight that already completed. */
export function useOriginFetch({
  repoPath,
  enabled,
  capabilityNonce,
  onFetched,
}: OriginFetchOptions): {
  state: FetchNowState;
  capability: RepoFetchCapability | null;
  runFetch: () => Promise<void>;
} {
  const [fetchState, dispatch] = useReducer(
    originFetchReducer,
    initialOriginFetchState,
  );
  const [capabilityEntry, setCapabilityEntry] =
    useState<CapabilityEntry | null>(null);
  const repoPathRef = useRef(repoPath);
  const enabledRef = useRef(enabled);
  const onFetchedRef = useRef(onFetched);
  const resultTimerRef = useRef<number | null>(null);
  const terminalFlightsRef = useRef(new Set<string>());
  repoPathRef.current = repoPath;
  enabledRef.current = enabled;
  onFetchedRef.current = onFetched;

  const capability =
    enabled &&
    repoPath &&
    capabilityEntry &&
    sameCanonicalPath(capabilityEntry.repoPath, repoPath)
      ? capabilityEntry.value
      : enabled && repoPath
        ? (cachedOriginCapability(repoPath) ?? null)
        : null;
  const capabilityRef = useRef(capability);
  capabilityRef.current = capability;

  const clearResultTimer = useCallback(() => {
    if (resultTimerRef.current != null) {
      window.clearTimeout(resultTimerRef.current);
      resultTimerRef.current = null;
    }
  }, []);

  const scheduleResultDecay = useCallback(
    (outcome: FetchRepoResult) => {
      if (outcome === "in_progress") return;
      clearResultTimer();
      resultTimerRef.current = window.setTimeout(
        () => {
          resultTimerRef.current = null;
          dispatch({ type: "decay" });
        },
        outcome === "busy" ? 2_600 : 4_000,
      );
    },
    [clearResultTimer],
  );

  const updateCapabilityFromOutcome = useCallback(
    (eventRepo: string, outcome: FetchRepoResult) => {
      const next =
        outcome === "no_origin"
          ? "no_origin"
          : outcome === "unavailable"
            ? "unavailable"
            : outcome === "ok"
              ? "available"
              : null;
      if (!next) return;
      rememberOriginCapability(eventRepo, next);
      const currentRepo = repoPathRef.current;
      if (currentRepo && sameCanonicalPath(currentRepo, eventRepo)) {
        setCapabilityEntry({ repoPath: currentRepo, value: next });
      }
    },
    [],
  );

  const markTerminalFlight = useCallback((flightId: string): boolean => {
    return acceptTerminalFlight(terminalFlightsRef.current, flightId);
  }, []);

  // Revalidate only on repository change or panel summon. A cached result
  // stays rendered while the local probe runs, avoiding capability flicker.
  useEffect(() => {
    if (!enabled || !repoPath) {
      setCapabilityEntry(null);
      return;
    }
    const cached = cachedOriginCapability(repoPath) ?? null;
    setCapabilityEntry((current) =>
      current && sameCanonicalPath(current.repoPath, repoPath)
        ? current
        : { repoPath, value: cached },
    );
    let cancelled = false;
    repoFetchCapability(repoPath)
      .then((next) => {
        rememberOriginCapability(repoPath, next);
        if (!cancelled) setCapabilityEntry({ repoPath, value: next });
      })
      .catch(() => {
        rememberOriginCapability(repoPath, "unavailable");
        if (!cancelled) {
          setCapabilityEntry({ repoPath, value: "unavailable" });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [capabilityNonce, enabled, repoPath]);

  // A result and any pending visual decay belong only to the repository that
  // initiated them.
  useEffect(() => {
    clearResultTimer();
    dispatch({ type: "repo_changed" });
  }, [clearResultTimer, repoPath]);

  // Auto-fetch and a manual join can outlive the command invocation. Listen
  // once and filter against refs so repository changes never receive stale
  // completion UI.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onFetchState((event) => {
      const currentRepo = repoPathRef.current;
      if (
        !enabledRef.current ||
        !currentRepo ||
        !sameCanonicalPath(currentRepo, event.repoPath)
      ) {
        return;
      }
      clearResultTimer();
      if (event.phase === "started") {
        dispatch({ type: "flight_started", flightId: event.flightId });
        return;
      }
      if (!markTerminalFlight(event.flightId)) return;
      dispatch({
        type: "flight_completed",
        flightId: event.flightId,
        outcome: event.outcome,
      });
      updateCapabilityFromOutcome(event.repoPath, event.outcome);
      if (event.outcome === "ok") onFetchedRef.current();
      scheduleResultDecay(event.outcome);
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // The desktop event bridge can be unavailable during SSR/tests. The
        // command path still reports a terminal result in that environment.
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [
    clearResultTimer,
    markTerminalFlight,
    scheduleResultDecay,
    updateCapabilityFromOutcome,
  ]);

  useEffect(() => clearResultTimer, [clearResultTimer]);

  const runFetch = useCallback(async () => {
    const requestedRepo = repoPathRef.current;
    if (
      !enabledRef.current ||
      !requestedRepo ||
      capabilityRef.current !== "available"
    ) {
      return;
    }

    clearResultTimer();
    dispatch({ type: "manual_started" });
    let result: FetchCommandResult;
    try {
      result = await fetchRepoNow(requestedRepo);
    } catch {
      result = { flightId: null, outcome: "failed" };
    }

    const currentRepo = repoPathRef.current;
    if (!currentRepo || !sameCanonicalPath(currentRepo, requestedRepo)) return;
    if (result.flightId && result.outcome !== "in_progress") {
      if (!markTerminalFlight(result.flightId)) return;
    }
    dispatch({ type: "command_completed", result });
    updateCapabilityFromOutcome(requestedRepo, result.outcome);
    if (result.outcome === "ok") onFetchedRef.current();
    if (result.outcome !== "in_progress") {
      scheduleResultDecay(result.outcome);
    }
  }, [
    clearResultTimer,
    markTerminalFlight,
    scheduleResultDecay,
    updateCapabilityFromOutcome,
  ]);

  return { state: fetchState.state, capability, runFetch };
}
