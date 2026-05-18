import { useEffect, useRef, type ReactNode } from "react";

interface Props {
  id: string;
  label: ReactNode;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  children: ReactNode;
  disabled?: boolean;
  title?: string;
  align?: "left" | "right";
}

export function ChipDropdown({
  id,
  label,
  open,
  onToggle,
  onClose,
  children,
  disabled,
  title,
  align = "left",
}: Props) {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    function handler(e: MouseEvent) {
      if (!ref.current) return;
      if (!ref.current.contains(e.target as Node)) onClose();
    }
    function key(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", handler);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("mousedown", handler);
      document.removeEventListener("keydown", key);
    };
  }, [open, onClose]);

  return (
    <div className="chip-wrap" data-chip={id} ref={ref}>
      <button
        type="button"
        className={"chip" + (open ? " open" : "") + (disabled ? " disabled" : "")}
        onClick={() => {
          if (!disabled) onToggle();
        }}
        disabled={disabled}
        title={title}
      >
        <span className="chip-label">{label}</span>
        <span className="chip-caret">▾</span>
      </button>
      {open && (
        <div className={"chip-dropdown chip-dropdown-" + align}>{children}</div>
      )}
    </div>
  );
}
