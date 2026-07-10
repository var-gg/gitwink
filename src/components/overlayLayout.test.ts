import { describe, expect, it } from "vitest";

import {
  constrainAnchoredOverlay,
  constrainFixedOverlay,
} from "./overlayLayout";

describe("constrainAnchoredOverlay", () => {
  for (const scale of [1, 1.3, 1.6]) {
    it(`keeps a right-aligned dropdown inside 360x420 at scale ${scale}`, () => {
      const layout = constrainAnchoredOverlay({
        anchorLeft: 278,
        anchorRight: 353,
        overlayTop: 62 * scale + 4 * scale,
        preferredWidth: 280 * scale,
        preferredMaxHeight: 320 * scale,
        viewport: { width: 360, height: 420 },
        align: "right",
      });
      const baseLeft = 353 - layout.width;
      const left = baseLeft + layout.shiftX;

      expect(left).toBeGreaterThanOrEqual(8);
      expect(left + layout.width).toBeLessThanOrEqual(352);
      expect(layout.maxHeight).toBeLessThanOrEqual(
        420 - 8 - (62 * scale + 4 * scale),
      );
      expect(layout.maxHeight).toBeGreaterThan(0);
    });
  }

  it("nudges a wide left-aligned repo dropdown off the panel edge", () => {
    const layout = constrainAnchoredOverlay({
      anchorLeft: 7,
      anchorRight: 130,
      overlayTop: 106,
      preferredWidth: 280 * 1.6,
      preferredMaxHeight: 320 * 1.6,
      viewport: { width: 360, height: 420 },
      align: "left",
    });
    const left = 7 + layout.shiftX;

    expect(left).toBe(8);
    expect(layout.width).toBe(344);
    expect(left + layout.width).toBe(352);
    expect(layout.maxHeight).toBe(306);
  });

  it("preserves the preferred size at the standard 520px layout", () => {
    const layout = constrainAnchoredOverlay({
      anchorLeft: 10,
      anchorRight: 200,
      overlayTop: 66,
      preferredWidth: 280,
      preferredMaxHeight: 320,
      viewport: { width: 520, height: 600 },
      align: "left",
    });

    expect(layout).toEqual({ width: 280, maxHeight: 320, shiftX: 0 });
  });
});

describe("constrainFixedOverlay", () => {
  it("clamps both edges when a scaled menu opens at the bottom-right", () => {
    expect(
      constrainFixedOverlay({
        x: 350,
        y: 410,
        overlayWidth: 320,
        overlayHeight: 300,
        viewport: { width: 360, height: 420 },
      }),
    ).toEqual({ x: 36, y: 116 });
  });
});
