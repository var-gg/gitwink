import { afterEach, describe, expect, it } from "vitest";

import {
  acceptTerminalFlight,
  cachedOriginCapability,
  clearOriginCapabilityCacheForTest,
  rememberOriginCapability,
} from "./useOriginFetch";

describe("origin capability cache", () => {
  afterEach(clearOriginCapabilityCacheForTest);

  it("keeps capability values keyed to their repository", () => {
    rememberOriginCapability("C:/repos/a", "available");
    rememberOriginCapability("C:/repos/b", "no_origin");
    expect(cachedOriginCapability("C:/repos/a")).toBe("available");
    expect(cachedOriginCapability("C:/repos/b")).toBe("no_origin");
    expect(cachedOriginCapability("C:/repos/c")).toBeUndefined();
  });

  it("matches alternate Windows path spelling without leaking another repo", () => {
    rememberOriginCapability("C:\\Repos\\Gitwink", "available");
    expect(cachedOriginCapability("c:/repos/gitwink/")).toBe("available");
    expect(cachedOriginCapability("c:/repos/gitwink-other/")).toBeUndefined();
  });

  it("accepts one terminal side effect per flight", () => {
    const seen = new Set<string>();
    expect(acceptTerminalFlight(seen, "f1")).toBe(true);
    expect(acceptTerminalFlight(seen, "f1")).toBe(false);
    expect(acceptTerminalFlight(seen, "f2")).toBe(true);
  });
});
