export interface ViewportSize {
  width: number;
  height: number;
}

export interface AnchoredOverlayInput {
  anchorLeft: number;
  anchorRight: number;
  overlayTop: number;
  preferredWidth: number;
  preferredMaxHeight: number;
  viewport: ViewportSize;
  align: "left" | "right";
  edgePadding?: number;
}

export interface AnchoredOverlayLayout {
  width: number;
  maxHeight: number;
  shiftX: number;
}

/**
 * Constrain an overlay that stays anchored to a trigger's left or right edge.
 * Width is reduced before the horizontal correction is calculated, so a
 * scaled overlay can never be wider than the viewport it is being nudged in.
 */
export function constrainAnchoredOverlay({
  anchorLeft,
  anchorRight,
  overlayTop,
  preferredWidth,
  preferredMaxHeight,
  viewport,
  align,
  edgePadding = 8,
}: AnchoredOverlayInput): AnchoredOverlayLayout {
  const availableWidth = Math.max(0, viewport.width - edgePadding * 2);
  const width = Math.max(0, Math.min(preferredWidth, availableWidth));
  const baseLeft = align === "left" ? anchorLeft : anchorRight - width;
  const maxLeft = Math.max(edgePadding, viewport.width - edgePadding - width);
  const clampedLeft = clamp(baseLeft, edgePadding, maxLeft);
  const availableHeight = Math.max(
    0,
    viewport.height - edgePadding - overlayTop,
  );

  return {
    width,
    maxHeight: Math.max(0, Math.min(preferredMaxHeight, availableHeight)),
    shiftX: clampedLeft - baseLeft,
  };
}

export interface FixedOverlayInput {
  x: number;
  y: number;
  overlayWidth: number;
  overlayHeight: number;
  viewport: ViewportSize;
  edgePadding?: number;
}

/** Clamp a fixed overlay's origin after CSS has constrained its dimensions. */
export function constrainFixedOverlay({
  x,
  y,
  overlayWidth,
  overlayHeight,
  viewport,
  edgePadding = 4,
}: FixedOverlayInput): { x: number; y: number } {
  const maxX = Math.max(edgePadding, viewport.width - overlayWidth - edgePadding);
  const maxY = Math.max(
    edgePadding,
    viewport.height - overlayHeight - edgePadding,
  );
  return {
    x: clamp(x, edgePadding, maxX),
    y: clamp(y, edgePadding, maxY),
  };
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}
