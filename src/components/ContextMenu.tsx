import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { useUiScale } from "../lib/settings";
import { constrainFixedOverlay } from "./overlayLayout";
import {
  DISMISS_CONTEXT_MENUS_EVENT,
  MODAL_OPEN_EVENT,
} from "./overlayLifecycle";

export interface MenuItem {
  label?: string;
  /** Accessible name override. Set when the visible label carries a
   *  decorative emoji a screen reader would otherwise announce. */
  ariaLabel?: string;
  /** Optional consequence/help text rendered as a quieter second line. */
  description?: string;
  title?: string;
  onClick?: () => void | Promise<void>;
  disabled?: boolean;
  divider?: boolean;
}

interface Props {
  items: MenuItem[];
  x: number;
  y: number;
  onClose: () => void;
  /** Explicit keyboard-return target when the opening gesture can identify
   * its invoker. Falls back to the element focused before the menu mounted. */
  restoreFocusTo?: HTMLElement | null;
}

const EDGE_PAD = 4;

/** Context menus are transient keyboard layers. Tab dismisses the layer and
 * returns focus to its invoker instead of letting focus escape underneath a
 * still-visible menu. Exported as a tiny contract for node-only tests. */
export function contextMenuDismissesOnTab(key: string): boolean {
  return key === "Tab";
}

export function ContextMenu({
  items,
  x,
  y,
  onClose,
  restoreFocusTo,
}: Props) {
  const menuRef = useRef<HTMLDivElement | null>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(
    restoreFocusTo ??
      (document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null),
  );
  if (restoreFocusTo != null) restoreFocusRef.current = restoreFocusTo;
  const scale = useUiScale();
  const [pos, setPos] = useState({ x, y });

  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    let frame = 0;
    const measure = () => {
      frame = 0;
      const rect = menu.getBoundingClientRect();
      const next = constrainFixedOverlay({
        x,
        y,
        overlayWidth: rect.width,
        overlayHeight: rect.height,
        viewport: { width: window.innerWidth, height: window.innerHeight },
        edgePadding: EDGE_PAD,
      });
      setPos((previous) =>
        previous.x === next.x && previous.y === next.y ? previous : next,
      );
    };
    const scheduleMeasure = () => {
      if (frame) window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(measure);
    };

    measure();
    window.addEventListener("resize", scheduleMeasure);
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(scheduleMeasure);
    observer?.observe(menu);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", scheduleMeasure);
      observer?.disconnect();
    };
  }, [x, y, scale]);

  useEffect(() => {
    const menu = menuRef.current;
    const first = menu?.querySelector<HTMLButtonElement>(
      "button:not(:disabled)",
    );
    (first ?? menu)?.focus({ preventScroll: true });
  }, []);

  function restoreFocus() {
    window.requestAnimationFrame(() => {
      const target = restoreFocusRef.current;
      if (target?.isConnected) target.focus({ preventScroll: true });
    });
  }

  useEffect(() => {
    function onDown(e: MouseEvent) {
      const target = e.target as HTMLElement | null;
      if (!target?.closest(".context-menu")) onClose();
    }
    function onKey(e: KeyboardEvent) {
      if (e.isComposing || e.keyCode === 229) return;
      // A true modal owns Escape and every other key. The modal-open event
      // normally unmounts us first; this guard closes the tiny same-frame gap.
      if (document.querySelector(".update-modal")) return;
      if (contextMenuDismissesOnTab(e.key)) {
        onClose();
        restoreFocus();
        // Closing is asynchronous from this native document listener. Do not
        // let the browser calculate its next tab stop from menu DOM that is
        // being removed; the next Tab starts safely from the restored owner.
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (e.key === "Escape") {
        onClose();
        restoreFocus();
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (!menuRef.current) return;
      const items = Array.from(
        menuRef.current.querySelectorAll<HTMLButtonElement>(
          'button:not(:disabled)',
        ),
      );
      if (items.length === 0) return;
      const current = items.indexOf(document.activeElement as HTMLButtonElement);
      let next: number | null = null;
      if (e.key === "ArrowDown") next = (current + 1) % items.length;
      else if (e.key === "ArrowUp")
        next = (current <= 0 ? items.length : current) - 1;
      else if (e.key === "Home") next = 0;
      else if (e.key === "End") next = items.length - 1;
      if (next == null) return;
      items[next].focus();
      e.preventDefault();
      e.stopPropagation();
    }
    function onModalOpen() {
      // Do not restore focus underneath the modal. UpdateModal moves focus to
      // its first action after broadcasting this signal.
      onClose();
    }
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener(MODAL_OPEN_EVENT, onModalOpen);
    window.addEventListener(DISMISS_CONTEXT_MENUS_EVENT, onModalOpen);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener(MODAL_OPEN_EVENT, onModalOpen);
      window.removeEventListener(DISMISS_CONTEXT_MENUS_EVENT, onModalOpen);
    };
  }, [onClose]);

  return (
    <div
      ref={menuRef}
      className="context-menu"
      role="menu"
      aria-label="Context menu"
      tabIndex={-1}
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
      data-no-drag
    >
      {items.map((item, i) =>
        item.divider ? (
          <div
            key={i}
            className="context-menu-divider"
            role="separator"
          />
        ) : (
          <button
            key={i}
            type="button"
            role="menuitem"
            className={
              "context-menu-item" + (item.disabled ? " disabled" : "")
            }
            aria-label={item.ariaLabel}
            title={item.title ?? item.label}
            disabled={item.disabled}
            onClick={() => {
              if (item.disabled) return;
              void item.onClick?.();
              onClose();
              restoreFocus();
            }}
          >
            {item.label}
            {item.description && (
              <span className="context-menu-description">
                {item.description}
              </span>
            )}
          </button>
        ),
      )}
    </div>
  );
}
