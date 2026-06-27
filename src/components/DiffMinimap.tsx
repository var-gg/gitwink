// Diff overview rail — the "where are the changes" scrollbar.
//
// A thin vertical strip on the right of a side-by-side diff that paints one
// tinted mark per add/delete/replace segment (positioned by fraction of the
// total row count). It also shows the current viewport:
//   - locked   → one thumb (both columns scroll together)
//   - unlocked → two thumbs, one per column, so you can SEE how far the old
//                and new sides have drifted apart without forcing them back.
//                A re-align button appears while they're out of sync.
// Clicking or dragging the rail scrolls BOTH columns to that position (which
// also re-aligns them). Wheel/keyboard scrolling on a column still works.

import { useEffect, useRef, useState } from "react";

import type { MinimapSegment } from "../lib/diffView";

export type { MinimapSegment };

interface Props {
  segments: MinimapSegment[];
  leftRef: React.RefObject<HTMLDivElement | null>;
  rightRef: React.RefObject<HTMLDivElement | null>;
  /** true = columns scroll as one; false = independent (show both thumbs) */
  locked: boolean;
}

const clamp01 = (n: number) => Math.min(1, Math.max(0, n));

interface Band {
  top: number;
  height: number;
  overflow: boolean;
}
const FULL: Band = { top: 0, height: 1, overflow: false };

export function DiffMinimap({ segments, leftRef, rightRef, locked }: Props) {
  const railRef = useRef<HTMLDivElement | null>(null);
  const draggingRef = useRef(false);
  const [left, setLeft] = useState<Band>(FULL);
  const [right, setRight] = useState<Band>(FULL);
  const [diverged, setDiverged] = useState(false);

  // Mirror both columns' scroll position/size into the thumbs. rAF-throttled
  // so a fast wheel scroll doesn't re-render per event; a ResizeObserver
  // catches viewport resizes and content-height changes (diff swap, async
  // highlight reflow) so the thumbs stay correct without a scroll.
  useEffect(() => {
    const l = leftRef.current;
    const r = rightRef.current;
    if (!l || !r) return;
    let raf = 0;
    const band = (el: HTMLDivElement): Band => {
      const overflow = el.scrollHeight > el.clientHeight + 1;
      return {
        top: overflow ? el.scrollTop / el.scrollHeight : 0,
        height: overflow ? el.clientHeight / el.scrollHeight : 1,
        overflow,
      };
    };
    const read = () => {
      raf = 0;
      setLeft(band(l));
      setRight(band(r));
      setDiverged(Math.abs(l.scrollTop - r.scrollTop) > 4);
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(read);
    };
    read();
    l.addEventListener("scroll", schedule, { passive: true });
    r.addEventListener("scroll", schedule, { passive: true });
    const ro = new ResizeObserver(schedule);
    ro.observe(l);
    ro.observe(r);
    if (l.firstElementChild) ro.observe(l.firstElementChild);
    return () => {
      l.removeEventListener("scroll", schedule);
      r.removeEventListener("scroll", schedule);
      ro.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
  }, [leftRef, rightRef]);

  // Scroll both columns so the rail position under `clientY` sits at their
  // viewport center. Fraction-based, so equal-height columns land aligned.
  function scrollBothToClientY(clientY: number) {
    const rail = railRef.current;
    if (!rail) return;
    const rect = rail.getBoundingClientRect();
    if (rect.height <= 0) return;
    const frac = clamp01((clientY - rect.top) / rect.height);
    for (const el of [leftRef.current, rightRef.current]) {
      if (!el) continue;
      el.scrollTop = Math.min(
        el.scrollHeight - el.clientHeight,
        Math.max(0, frac * el.scrollHeight - el.clientHeight / 2),
      );
    }
  }

  function onPointerDown(e: React.PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    draggingRef.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
    scrollBothToClientY(e.clientY);
  }
  function onPointerMove(e: React.PointerEvent) {
    if (!draggingRef.current) return;
    scrollBothToClientY(e.clientY);
  }
  function endDrag(e: React.PointerEvent) {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {}
  }

  // Snap the new (right) side back onto the old (left) side's position.
  function realign(e: React.MouseEvent) {
    e.stopPropagation();
    const l = leftRef.current;
    const r = rightRef.current;
    if (l && r) r.scrollTop = l.scrollTop;
  }

  const showRealign = !locked && diverged;

  return (
    <div
      className="sbs-minimap"
      ref={railRef}
      role="scrollbar"
      aria-orientation="vertical"
      aria-label="Diff overview — click or drag to jump to a change"
      title="Diff overview — click or drag to jump to a change"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
    >
      {segments.map((s, i) => (
        <div
          key={i}
          className={`sbs-minimap-mark ${s.type}`}
          style={{ top: `${s.topPct}%`, height: `${s.heightPct}%` }}
        />
      ))}

      {locked
        ? left.overflow && (
            <div
              className="sbs-minimap-thumb"
              style={{
                top: `${left.top * 100}%`,
                height: `${left.height * 100}%`,
              }}
            />
          )
        : (
          <>
            {left.overflow && (
              <div
                className="sbs-minimap-thumb dual old"
                title="Old (before) viewport"
                style={{
                  top: `${left.top * 100}%`,
                  height: `${left.height * 100}%`,
                }}
              />
            )}
            {right.overflow && (
              <div
                className="sbs-minimap-thumb dual new"
                title="New (after) viewport"
                style={{
                  top: `${right.top * 100}%`,
                  height: `${right.height * 100}%`,
                }}
              />
            )}
          </>
        )}

      {showRealign && (
        <button
          type="button"
          className="sbs-minimap-realign"
          title="Re-align the two sides"
          aria-label="Re-align the two sides"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={realign}
        >
          ⇄
        </button>
      )}
    </div>
  );
}
