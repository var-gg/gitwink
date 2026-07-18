import { describe, expect, it } from "vitest";

import {
  initialOriginFetchState,
  originFetchReducer,
} from "./originFetchState";

describe("origin fetch flight state", () => {
  it("keeps a joined automatic flight active until its completion event", () => {
    let state = originFetchReducer(initialOriginFetchState, {
      type: "manual_started",
    });
    state = originFetchReducer(state, {
      type: "command_completed",
      result: { flightId: "f1", outcome: "in_progress" },
    });
    expect(state.state).toBe("in_progress");
    expect(originFetchReducer(state, { type: "decay" })).toEqual(state);

    state = originFetchReducer(state, {
      type: "flight_completed",
      flightId: "f1",
      outcome: "ok",
    });
    expect(state.state).toBe("ok");
  });

  it("does not let a late in-progress response regress a completed event", () => {
    let state = originFetchReducer(initialOriginFetchState, {
      type: "flight_completed",
      flightId: "f2",
      outcome: "ok",
    });
    state = originFetchReducer(state, {
      type: "command_completed",
      result: { flightId: "f2", outcome: "in_progress" },
    });
    expect(state.state).toBe("ok");
  });

  it("retains completion identity across visual decay", () => {
    let state = originFetchReducer(initialOriginFetchState, {
      type: "flight_completed",
      flightId: "f3",
      outcome: "network",
    });
    state = originFetchReducer(state, { type: "decay" });
    expect(state.state).toBe("idle");
    expect(state.completedFlightId).toBe("f3");
    expect(
      originFetchReducer(state, {
        type: "command_completed",
        result: { flightId: "f3", outcome: "network" },
      }),
    ).toEqual(state);
  });

  it("clears every flight identity when the viewed repository changes", () => {
    const completed = originFetchReducer(initialOriginFetchState, {
      type: "flight_completed",
      flightId: "f4",
      outcome: "ok",
    });
    expect(
      originFetchReducer(completed, { type: "repo_changed" }),
    ).toEqual(initialOriginFetchState);
  });

  it("allows a global busy result to decay and be retried", () => {
    let state = originFetchReducer(initialOriginFetchState, {
      type: "command_completed",
      result: { flightId: null, outcome: "busy" },
    });
    expect(state.state).toBe("busy");
    state = originFetchReducer(state, { type: "decay" });
    expect(state.state).toBe("idle");
  });
});
