import { describe, expect, it } from "vitest";

import {
  classifyPanelViewport,
  effectivePanelWidth,
} from "./usePanelViewportClass";

describe("panel viewport classification", () => {
  it.each([
    [379, "compact"],
    [420, "compact"],
    [459, "compact"],
    [460, "regular"],
    [719, "regular"],
    [720, "wide"],
    [1199, "wide"],
    [1200, "expansive"],
  ] as const)("classifies %i effective pixels as %s", (width, expected) => {
    expect(classifyPanelViewport(width, 1)).toBe(expected);
  });

  it("divides raw width by UI scale before choosing a tier", () => {
    expect(effectivePanelWidth(736, 1.6)).toBe(460);
    expect(classifyPanelViewport(736, 1.6)).toBe("regular");
    expect(classifyPanelViewport(735, 1.6)).toBe("compact");
  });

  it("keeps small maximized displays within their actual information tier", () => {
    expect(classifyPanelViewport(1024, 1)).toBe("wide");
    expect(classifyPanelViewport(1920, 1.6)).toBe("expansive");
  });

  it("handles invalid measurements defensively", () => {
    expect(effectivePanelWidth(Number.NaN, 0)).toBe(0);
    expect(classifyPanelViewport(Number.NaN, 0)).toBe("compact");
  });
});
