import { describe, expect, it } from "vitest";

import { filterRollingWindow, nextRollingWindowExpiry } from "./rollingWindow";

describe("rolling single-repo window", () => {
  it("matches the backend's inclusive cutoff exactly", () => {
    const now = 1_000_000;
    const cutoff = now - 86_400;
    const commits = [
      { timestamp: cutoff - 1, id: "expired" },
      { timestamp: cutoff, id: "boundary" },
      { timestamp: now, id: "new" },
    ];

    expect(filterRollingWindow(commits, 1, now).map((item) => item.id)).toEqual([
      "boundary",
      "new",
    ]);
  });

  it("schedules the precise second after the oldest row leaves scope", () => {
    const now = 1_000_000;
    const oldest = now - 100;
    expect(nextRollingWindowExpiry([{ timestamp: oldest }], 1, now)).toBe(
      oldest + 86_400 + 1,
    );
  });

  it("does no allocation or expiry work for all-time scope", () => {
    const commits = [{ timestamp: 1 }];
    expect(filterRollingWindow(commits, "all", 10_000)).toBe(commits);
  });

  it("preserves the snapshot identity when every row remains visible", () => {
    const commits = [{ timestamp: 999_999 }];
    expect(filterRollingWindow(commits, 1, 1_000_000)).toBe(commits);
  });
});
