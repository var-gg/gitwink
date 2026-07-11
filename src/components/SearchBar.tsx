// Commit search bar — the `/` summon.
//
// The bar owns only the input, the match count, and the keyboard bridge:
// the timeline body below it IS the result list (windowed, query-filtered),
// so ↑/↓/Enter are forwarded to its selection while focus stays here.
// Esc closes the bar (stopPropagation keeps App's Esc cascade from also
// firing on the same keypress).

import { useEffect, useRef } from "react";
import { useLingui } from "@lingui/react/macro";
import { filterMessages } from "../i18n/messages/filters";
import { translate } from "../i18n/translate";

export const SEARCH_QUERY_MAX_LENGTH = 512;

interface Props {
  /** live input text (debounced upstream into the actual query) */
  value: string;
  /** filtered result count, or null while loading / no query */
  count: number | null;
  /** bumped to re-focus the input (pressing `/` while the bar is open) */
  focusNonce: number;
  /** Explicit query scope. Search intentionally ignores time/author/branch
   * lenses, so this label prevents the hidden-mode semantic mismatch. */
  scopeLabel: string;
  /** Stable id of the timeline listbox controlled by this combobox. */
  resultsListId: string;
  /** True only while that listbox is mounted for the debounced query. */
  resultsAvailable: boolean;
  /** Result resolution is in flight; stale rows are deliberately inactive. */
  resultsBusy: boolean;
  /** Id of the selected, mounted option. Null while pending/unloaded. */
  activeDescendant: string | null;
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
  scopeLabel,
  resultsListId,
  resultsAvailable,
  resultsBusy,
  activeDescendant,
  onChange,
  onClose,
  onMove,
  onActivate,
}: Props) {
  const { i18n } = useLingui();
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
        maxLength={SEARCH_QUERY_MAX_LENGTH}
        value={value}
        placeholder={i18n._(filterMessages.searchCommitsPlaceholder)}
        spellCheck={false}
        aria-label={i18n._(filterMessages.searchCommitsAria)}
        role="combobox"
        aria-autocomplete="list"
        aria-haspopup="listbox"
        aria-controls={resultsAvailable ? resultsListId : undefined}
        aria-expanded={resultsAvailable}
        aria-busy={resultsBusy}
        aria-activedescendant={
          resultsAvailable && !resultsBusy
            ? activeDescendant ?? undefined
            : undefined
        }
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          // IME (e.g. Hangul) composition commits with Enter and cancels
          // with Esc — neither must warp or close the bar mid-word.
          if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) {
            return;
          }
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onClose();
            window.requestAnimationFrame(() => {
              document
                .querySelector<HTMLButtonElement>(".panel-search-btn")
                ?.focus({ preventScroll: true });
            });
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
          aria-label={i18n._(filterMessages.clearSearch)}
          title={i18n._(filterMessages.clearSearch)}
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
          {count == null
            ? "…"
            : translate(i18n, filterMessages.searchMatchCount, { count })}
        </span>
      )}
      <span
        className="search-bar-scope"
        title={translate(i18n, filterMessages.searchScope, {
          scope: scopeLabel,
        })}
      >
        {scopeLabel}
      </span>
      {count != null && count > 0 && (
        <span
          className="search-bar-hint"
          title={i18n._(filterMessages.searchKeyboardHint)}
        >
          ↵ {i18n._(filterMessages.jumpToHistory)}
        </span>
      )}
    </div>
  );
}
