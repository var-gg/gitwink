import { useEffect, useLayoutEffect, useRef, useState } from "react";

export interface MenuItem {
  label?: string;
  /** Accessible name override. Set when the visible label carries a
   *  decorative emoji a screen reader would otherwise announce. */
  ariaLabel?: string;
  /** Optional consequence/help text rendered as a quieter second line. */
  description?: string;
  onClick?: () => void | Promise<void>;
  disabled?: boolean;
  divider?: boolean;
}

interface Props {
  items: MenuItem[];
  x: number;
  y: number;
  onClose: () => void;
}

const EDGE_PAD = 4;

export function ContextMenu({ items, x, y, onClose }: Props) {
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ x, y });

  useLayoutEffect(() => {
    if (!menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    let nx = x;
    let ny = y;
    if (nx + rect.width > window.innerWidth) {
      nx = Math.max(EDGE_PAD, window.innerWidth - rect.width - EDGE_PAD);
    }
    if (ny + rect.height > window.innerHeight) {
      ny = Math.max(EDGE_PAD, window.innerHeight - rect.height - EDGE_PAD);
    }
    setPos({ x: nx, y: ny });
  }, [x, y]);

  useEffect(() => {
    menuRef.current
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
  }, []);

  useEffect(() => {
    function onDown(e: MouseEvent) {
      const target = e.target as HTMLElement | null;
      if (!target?.closest(".context-menu")) onClose();
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        onClose();
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
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  return (
    <div
      ref={menuRef}
      className="context-menu"
      role="menu"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
      data-no-drag
    >
      {items.map((item, i) =>
        item.divider ? (
          <div key={i} className="context-menu-divider" />
        ) : (
          <button
            key={i}
            type="button"
            role="menuitem"
            className={
              "context-menu-item" + (item.disabled ? " disabled" : "")
            }
            aria-label={item.ariaLabel}
            disabled={item.disabled}
            onClick={() => {
              if (item.disabled) return;
              void item.onClick?.();
              onClose();
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
