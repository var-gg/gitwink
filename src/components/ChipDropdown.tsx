import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { useLingui } from "@lingui/react/macro";

import { useUiScale } from "../lib/settings";
import { constrainAnchoredOverlay } from "./overlayLayout";
import { MODAL_OPEN_EVENT } from "./overlayLifecycle";
import { filterMessages } from "../i18n/messages/filters";

const DROPDOWN_WIDTH_BASE = 280;
const DROPDOWN_MAX_HEIGHT_BASE = 320;
const EDGE_PAD = 8;

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
  /** The chip is narrowing the view (a non-default filter). Paints a
   *  persistent accent so a short/empty list's cause is one glance away. */
  active?: boolean;
  /** Optional companion action rendered as a real sibling button. This is
   *  deliberately not nested inside the disclosure button. */
  onClear?: () => void;
  clearTitle?: string;
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
  active,
  onClear,
  clearTitle,
}: Props) {
  const { i18n } = useLingui();
  const ref = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const dropdownRef = useRef<HTMLDivElement | null>(null);
  const scale = useUiScale();
  const [layout, setLayout] = useState({
    width: DROPDOWN_WIDTH_BASE,
    maxHeight: DROPDOWN_MAX_HEIGHT_BASE,
    shiftX: 0,
  });
  const triggerId = `chip-trigger-${id}`;
  const dropdownId = `chip-dropdown-${id}`;
  const wasOpenRef = useRef(open);

  useLayoutEffect(() => {
    const wasOpen = wasOpenRef.current;
    wasOpenRef.current = open;
    if (!wasOpen || open) return;
    const active = document.activeElement as HTMLElement | null;
    // Option selection unmounts the focused row. Restore the surviving
    // disclosure trigger, but never steal focus from an outside-click target.
    if (!active || active === document.body || !active.isConnected) {
      window.requestAnimationFrame(() => {
        triggerRef.current?.focus({ preventScroll: true });
      });
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function handler(e: MouseEvent) {
      if (!ref.current) return;
      if (ref.current.contains(e.target as Node)) return;
      // Repo row actions render as a fixed sibling so dropdown transforms do
      // not skew their coordinates. The action menu still belongs to this
      // dropdown layer and must not trigger an outside close.
      if ((e.target as HTMLElement | null)?.closest?.(".context-menu")) return;
      onClose();
    }
    function key(e: KeyboardEvent) {
      // The Esc that cancels an IME composition (in the chip's search input)
      // must not close the dropdown.
      if (e.isComposing || e.keyCode === 229) return;
      if (e.key === "Escape") {
        // The update modal is the topmost layer — it must win Esc even if a
        // dropdown was left open beneath it.
        if (document.querySelector(".update-modal")) return;
        if (document.querySelector(".context-menu")) return;
        onClose();
        window.requestAnimationFrame(() => {
          triggerRef.current?.focus({ preventScroll: true });
        });
        // One Esc = one layer: without this, the same keypress fell through
        // to App's cascade and ALSO collapsed the open commit expansion —
        // the only overlay that leaked (ContextMenu/SearchBar/find all stop).
        e.preventDefault();
        e.stopPropagation();
      }
    }
    function modalOpen() {
      onClose();
    }
    document.addEventListener("mousedown", handler);
    document.addEventListener("keydown", key);
    window.addEventListener(MODAL_OPEN_EVENT, modalOpen);
    return () => {
      document.removeEventListener("mousedown", handler);
      document.removeEventListener("keydown", key);
      window.removeEventListener(MODAL_OPEN_EVENT, modalOpen);
    };
  }, [open, onClose]);

  // A scaled dropdown can be wider than the entire panel and its list can be
  // taller than the space left under the two-row header. Size it first, then
  // correct the anchored edge. ResizeObserver covers content/font changes;
  // the window listener and `scale` dependency cover viewport/live settings.
  useLayoutEffect(() => {
    if (!open) return;
    const el = dropdownRef.current;
    const anchor = ref.current;
    if (!el || !anchor) return;

    let frame = 0;
    const measure = () => {
      frame = 0;
      const overlayRect = el.getBoundingClientRect();
      const anchorRect = anchor.getBoundingClientRect();
      const next = constrainAnchoredOverlay({
        anchorLeft: anchorRect.left,
        anchorRight: anchorRect.right,
        overlayTop: overlayRect.top,
        preferredWidth: DROPDOWN_WIDTH_BASE * scale,
        preferredMaxHeight: DROPDOWN_MAX_HEIGHT_BASE * scale,
        viewport: { width: window.innerWidth, height: window.innerHeight },
        align,
        edgePadding: EDGE_PAD,
      });
      setLayout((previous) =>
        previous.width === next.width &&
        previous.maxHeight === next.maxHeight &&
        previous.shiftX === next.shiftX
          ? previous
          : next,
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
    observer?.observe(anchor);
    observer?.observe(el);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", scheduleMeasure);
      observer?.disconnect();
    };
  }, [open, align, scale]);

  // When the label itself is a string, surface it as the button title so
  // the truncated/ellipsised text still has a hover-reveal — caller can
  // still override with an explicit `title` prop.
  const effectiveTitle =
    title ?? (typeof label === "string" ? label : undefined);
  const effectiveClearTitle = clearTitle ?? i18n._(filterMessages.clearFilter);
  const dropdownStyle: CSSProperties = {
    width: layout.width,
    maxHeight: layout.maxHeight,
    transform: layout.shiftX
      ? `translateX(${layout.shiftX}px)`
      : undefined,
  };

  const trigger = (
    <button
      ref={triggerRef}
      id={triggerId}
      type="button"
      className={
        "chip" +
        (open ? " open" : "") +
        (active ? " active" : "") +
        (disabled ? " disabled" : "") +
        (onClear ? " chip-with-clear" : "")
      }
      onClick={() => {
        if (!disabled) onToggle();
      }}
      disabled={disabled}
      title={effectiveTitle}
      aria-expanded={open}
      aria-controls={open ? dropdownId : undefined}
      aria-haspopup="dialog"
    >
      <span className="chip-label">{label}</span>
      <span className="chip-caret" aria-hidden="true">
        ▾
      </span>
    </button>
  );

  return (
    <div className="chip-wrap" data-chip={id} ref={ref}>
      {onClear ? (
        <div className="chip-control" role="group" aria-label={effectiveTitle}>
          {trigger}
          <button
            type="button"
            className={
              "chip-clear-button" +
              (open ? " open" : "") +
              (active ? " active" : "")
            }
            onClick={() => {
              onClear();
              window.requestAnimationFrame(() => {
                triggerRef.current?.focus({ preventScroll: true });
              });
            }}
            title={effectiveClearTitle}
            aria-label={effectiveClearTitle}
          >
            ✕
          </button>
        </div>
      ) : (
        trigger
      )}
      {open && (
        <div
          id={dropdownId}
          ref={dropdownRef}
          className={"chip-dropdown chip-dropdown-" + align}
          style={dropdownStyle}
          role="dialog"
          aria-labelledby={triggerId}
        >
          {children}
        </div>
      )}
    </div>
  );
}
