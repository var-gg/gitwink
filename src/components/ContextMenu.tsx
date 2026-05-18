import { useEffect, useLayoutEffect, useRef, useState } from "react";

export interface MenuItem {
  label?: string;
  onClick?: () => void;
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
    function onDown(e: MouseEvent) {
      const target = e.target as HTMLElement | null;
      if (!target?.closest(".context-menu")) onClose();
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        onClose();
        e.preventDefault();
        e.stopPropagation();
      }
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
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) =>
        item.divider ? (
          <div key={i} className="context-menu-divider" />
        ) : (
          <button
            key={i}
            type="button"
            className={
              "context-menu-item" + (item.disabled ? " disabled" : "")
            }
            disabled={item.disabled}
            onClick={() => {
              if (item.disabled) return;
              item.onClick?.();
              onClose();
            }}
          >
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}
