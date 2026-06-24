// Commit search bar — the `/` summon.
//
// The bar owns only the input, the match count, and the keyboard bridge:
// the timeline body below it IS the result list (windowed, query-filtered),
// so ↑/↓/Enter are forwarded to its selection while focus stays here.
// Esc closes the bar (stopPropagation keeps App's Esc cascade from also
// firing on the same keypress).

import { useEffect, useRef } from "react";

interface Props {
  /** live input text (debounced upstream into the actual query) */
  value: string;
  /** filtered result count, or null while loading / no query */
  count: number | null;
  /** bumped to re-focus the input (pressing `/` while the bar is open) */
  focusNonce: number;
  onChange: (v: string) => void;
  onClose: () => void;
  /** move the result selection (↑/↓ from the input) */
  onMove: (delta: number) => void;
  /** warp to the selected result (Enter from the input) */
  onActivate: () => void;
}

export function SearchBar({
  value,
  count,
  focusNonce,
  onChange,
  onClose,
  onMove,
  onActivate,
}: Props) {
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [focusNonce]);

  return (
    <div className="search-bar">
      <span className="search-bar-icon" aria-hidden="true">
        ⌕
      </span>
      <input
        ref={inputRef}
        className="search-bar-input"
        type="text"
        value={value}
        placeholder="Search commits — message, author, SHA prefix…"
        spellCheck={false}
        aria-label="Search commits"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onClose();
          } else if (e.key === "ArrowDown") {
            e.preventDefault();
            onMove(1);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            onMove(-1);
          } else if (e.key === "Enter") {
            e.preventDefault();
            onActivate();
          }
        }}
      />
      {value !== "" && (
        <button
          type="button"
          className="search-bar-clear"
          aria-label="Clear search"
          title="Clear search"
          // preventDefault on mousedown keeps focus in the input (so the
          // bar doesn't blur-close), then we re-focus after clearing.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            onChange("");
            inputRef.current?.focus();
          }}
        >
          ✕
        </button>
      )}
      {value.trim() !== "" && (
        <span className="search-bar-count" aria-live="polite">
          {count == null ? "…" : `${count} match${count === 1 ? "" : "es"}`}
        </span>
      )}
      <span
        className="search-bar-hint"
        title="↑↓ select · Enter jump to its history · click a row to preview · Esc close"
      >
        ↵ jump
      </span>
    </div>
  );
}
