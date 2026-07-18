import type { FetchCommandResult, FetchRepoResult } from "../lib/ipc";

export type FetchNowState = "idle" | "fetching" | FetchRepoResult;

export interface OriginFetchUiState {
  state: FetchNowState;
  flightId: string | null;
  completedFlightId: string | null;
}

export type OriginFetchAction =
  | { type: "repo_changed" }
  | { type: "manual_started" }
  | { type: "flight_started"; flightId: string }
  | { type: "flight_completed"; flightId: string; outcome: FetchRepoResult }
  | { type: "command_completed"; result: FetchCommandResult }
  | { type: "decay" };

export const initialOriginFetchState: OriginFetchUiState = {
  state: "idle",
  flightId: null,
  completedFlightId: null,
};

/** Pure flight state machine. Keeping terminal flight identity after visual
 * decay prevents a late command response from regressing a completed event
 * back to `in_progress`. */
export function originFetchReducer(
  current: OriginFetchUiState,
  action: OriginFetchAction,
): OriginFetchUiState {
  switch (action.type) {
    case "repo_changed":
      return initialOriginFetchState;
    case "manual_started":
      return { ...current, state: "fetching", flightId: null };
    case "flight_started":
      if (current.completedFlightId === action.flightId) return current;
      return { ...current, state: "fetching", flightId: action.flightId };
    case "flight_completed":
      if (current.completedFlightId === action.flightId) return current;
      return {
        state: action.outcome,
        flightId: action.flightId,
        completedFlightId: action.flightId,
      };
    case "command_completed": {
      const { flightId, outcome } = action.result;
      if (flightId && current.completedFlightId === flightId) return current;
      if (outcome === "in_progress") {
        return { ...current, state: outcome, flightId };
      }
      return {
        state: outcome,
        flightId,
        completedFlightId: flightId ?? current.completedFlightId,
      };
    }
    case "decay":
      if (current.state === "fetching" || current.state === "in_progress") {
        return current;
      }
      return { ...current, state: "idle", flightId: null };
  }
}
