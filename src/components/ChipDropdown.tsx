import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

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
  const dropdownRef = useRef<HTMLDivElement | null>(null);
  const [shift, setShift] = useState(0);

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

  // `align` only picks which edge to anchor to; on the fixed-width panel a
  // dropdown can still spill past the opposite window edge. Once open,
  // measure it and nudge it back inside with a transform.
  useLayoutEffect(() => {
    if (!open) return;
    const el = dropdownRef.current;
    if (!el) return;
    const pad = 8;
    const rect = el.getBoundingClientRect();
    const baseLeft = rect.left - shift;
    const baseRight = rect.right - shift;
    let dx = 0;
    if (baseLeft < pad) dx = pad - baseLeft;
    else if (baseRight > window.innerWidth - pad)
      dx = window.innerWidth - pad - baseRight;
    if (dx !== shift) setShift(dx);
  }, [open, align, shift]);

  // When the label itself is a string, surface it as the button title so
  // the truncated/ellipsised text still has a hover-reveal — caller can
  // still override with an explicit `title` prop.
  const effectiveTitle =
    title ?? (typeof label === "string" ? label : undefined);

  return (
    <div className="chip-wrap" data-chip={id} ref={ref}>
      <button
        type="button"
        className={"chip" + (open ? " open" : "") + (disabled ? " disabled" : "")}
        onClick={() => {
          if (!disabled) onToggle();
        }}
        disabled={disabled}
        title={effectiveTitle}
      >
        <span className="chip-label">{label}</span>
        <span className="chip-caret">▾</span>
      </button>
      {open && (
        <div
          ref={dropdownRef}
          className={"chip-dropdown chip-dropdown-" + align}
          style={shift ? { transform: `translateX(${shift}px)` } : undefined}
        >
          {children}
        </div>
      )}
    </div>
  );
}
