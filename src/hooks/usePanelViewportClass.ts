import { useEffect, useState } from "react";

import { useUiScale } from "../lib/settings";

export type PanelViewportClass =
  | "compact"
  | "regular"
  | "wide"
  | "expansive";

export function effectivePanelWidth(rawWidth: number, uiScale: number): number {
  const safeWidth = Number.isFinite(rawWidth) && rawWidth > 0 ? rawWidth : 0;
  const safeScale = Number.isFinite(uiScale) && uiScale > 0 ? uiScale : 1;
  return safeWidth / safeScale;
}

export function classifyPanelViewport(
  rawWidth: number,
  uiScale: number,
): PanelViewportClass {
  const width = effectivePanelWidth(rawWidth, uiScale);
  if (width < 460) return "compact";
  if (width < 720) return "regular";
  if (width < 1200) return "wide";
  return "expansive";
}

/** Width tier based on information space rather than raw window pixels. One
 * animation-frame update per resize burst keeps layout responsive without
 * coupling geometry changes to Git/cache queries. */
export function usePanelViewportClass(): PanelViewportClass {
  const uiScale = useUiScale();
  const [viewport, setViewport] = useState<PanelViewportClass>(() =>
    classifyPanelViewport(window.innerWidth, uiScale),
  );

  useEffect(() => {
    let frame = 0;
    const update = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        setViewport(classifyPanelViewport(window.innerWidth, uiScale));
      });
    };
    update();
    window.addEventListener("resize", update, { passive: true });
    return () => {
      window.removeEventListener("resize", update);
      window.cancelAnimationFrame(frame);
    };
  }, [uiScale]);

  return viewport;
}
