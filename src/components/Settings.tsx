import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  broadcastSettings,
  getCurrentSettings,
  type UpdateCheckMode,
} from "../lib/settings";

/** UI-scale slider bounds — mirror UI_SCALE_MIN/MAX in commands.rs. 100%
 *  is the floor: the diff/timeline default is the most compact legible
 *  size, so the control only scales up. */
const SCALE_MIN = 1;
const SCALE_MAX = 1.6;
const SCALE_STEP = 0.05;
/** Debounce before persisting — a slider sweep / font typing becomes
 *  one disk write per pause instead of one per tick / keystroke. */
const PERSIST_DELAY_MS = 250;

/** Common Windows monospace fonts as datalist suggestions. */
const FONT_PRESETS = ["Cascadia Code", "Cascadia Mono", "Consolas", "Courier New"];

/** Translate a KeyboardEvent into a Tauri accelerator spec, or null if
 *  the press isn't a valid binding (no modifier, only a modifier, or an
 *  unmapped key). Uses event.code (physical key) so it's layout- and
 *  IME-independent. */
function keyEventToAccelerator(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  if (mods.length === 0) return null;
  const key = codeToTauriKey(e.code);
  if (!key) return null;
  return [...mods, key].join("+");
}

function codeToTauriKey(code: string): string | null {
  // KeyA..KeyZ → A..Z (already uppercase letters).
  if (code.startsWith("Key") && code.length === 4) return code.slice(3);
  // Digit0..Digit9 → 0..9.
  if (code.startsWith("Digit") && code.length === 6) return code.slice(5);
  // F1..F24.
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const map: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Tab: "Tab",
    Backquote: "Backquote",
    Minus: "Minus",
    Equal: "Equal",
    BracketLeft: "BracketLeft",
    BracketRight: "BracketRight",
    Backslash: "Backslash",
    Semicolon: "Semicolon",
    Quote: "Quote",
    Comma: "Comma",
    Period: "Period",
    Slash: "Slash",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    ArrowUp: "Up",
    ArrowDown: "Down",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
    Delete: "Delete",
  };
  return map[code] ?? null;
}

export function Settings() {
  const [settings, setSettings] = useState(getCurrentSettings);
  const scaleTimer = useRef<number | undefined>(undefined);
  const fontTimer = useRef<number | undefined>(undefined);

  const [recording, setRecording] = useState(false);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  // Mirror settings into a ref so the recording effect doesn't have to
  // re-subscribe to keydown whenever an unrelated setting changes.
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  function setScale(uiScale: number) {
    const next = { ...settings, uiScale };
    setSettings(next);
    broadcastSettings(next);
    window.clearTimeout(scaleTimer.current);
    scaleTimer.current = window.setTimeout(() => {
      void invoke("set_ui_scale", { scale: uiScale });
    }, PERSIST_DELAY_MS);
  }

  function setFont(family: string) {
    const trimmed = family.trim();
    const fam = trimmed.length > 0 ? trimmed : null;
    const next = { ...settings, diffFontFamily: fam };
    setSettings(next);
    broadcastSettings(next);
    window.clearTimeout(fontTimer.current);
    fontTimer.current = window.setTimeout(() => {
      void invoke("set_diff_font", { family: fam });
    }, PERSIST_DELAY_MS);
  }

  function setUpdateMode(mode: UpdateCheckMode) {
    const next = { ...settings, updateCheck: mode };
    setSettings(next);
    broadcastSettings(next);
    // Persist immediately — radio clicks are single events, not a sweep,
    // so debounce buys nothing and the user expects the tray dot /
    // "Check for updates" item to react right away.
    void invoke("set_update_check", { mode });
  }

  function setAutoFetch(enabled: boolean) {
    const next = { ...settings, autoFetchOnShow: enabled };
    setSettings(next);
    broadcastSettings(next);
    // A checkbox is a single discrete event — persist immediately, no debounce.
    void invoke("set_auto_fetch_on_show", { enabled });
  }

  function openSettingsFile() {
    void invoke("open_settings_file").catch((err) => {
      // Surface in console for triage; the user already gets OS-level
      // feedback if the editor fails to launch.
      // eslint-disable-next-line no-console
      console.error("[gitwink] open_settings_file failed", err);
    });
  }

  // When the Settings window is hidden (X-click on close-on-hide path)
  // or the page is about to unload, drain any in-flight scale / font
  // debounce so a fast quit can't lose the just-edited value to disk
  // (GPT Pro review D2). LiveSettings already keeps the in-session
  // state correct across windows; this is the durability backstop for
  // values that hadn't yet been persisted when the user walked away.
  useEffect(() => {
    function flushPending() {
      if (scaleTimer.current !== undefined) {
        window.clearTimeout(scaleTimer.current);
        scaleTimer.current = undefined;
        void invoke("set_ui_scale", { scale: settingsRef.current.uiScale });
      }
      if (fontTimer.current !== undefined) {
        window.clearTimeout(fontTimer.current);
        fontTimer.current = undefined;
        void invoke("set_diff_font", {
          family: settingsRef.current.diffFontFamily,
        });
      }
    }
    function onVisibilityChange() {
      if (document.visibilityState === "hidden") flushPending();
    }
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("beforeunload", flushPending);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("beforeunload", flushPending);
    };
  }, []);

  // Recording mode: capture the next valid combo and send it to Rust to
  // re-bind the global shortcut live. Esc cancels. OS-reserved combos
  // (Alt+Tab, Win+L, …) never reach the webview, so the recorder simply
  // doesn't react to them — that's the right behaviour.
  useEffect(() => {
    if (!recording) return;
    function onKeyDown(e: KeyboardEvent) {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setRecording(false);
        setHotkeyError(null);
        return;
      }
      // Pure modifier presses don't commit — wait for the actual key.
      if (
        e.key === "Control" ||
        e.key === "Shift" ||
        e.key === "Alt" ||
        e.key === "Meta"
      ) {
        return;
      }
      const accel = keyEventToAccelerator(e);
      if (!accel) {
        setHotkeyError(
          "Need at least one modifier (Ctrl / Alt / Shift) + a key.",
        );
        return;
      }
      void (async () => {
        try {
          await invoke("set_panel_hotkey", { spec: accel });
          const next = { ...settingsRef.current, panelHotkey: accel };
          setSettings(next);
          broadcastSettings(next);
          setRecording(false);
          setHotkeyError(null);
        } catch (err) {
          // Most common case: the combo is already held by another app
          // (Windows registers globally, first-bind wins). Surface inline.
          setHotkeyError(String(err));
        }
      })();
    }
    document.addEventListener("keydown", onKeyDown, { capture: true });
    return () =>
      document.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [recording]);

  return (
    <div className="settings">
      <h1 className="settings-title">Settings</h1>

      <section className="settings-section">
        <h2 className="settings-section-title">Appearance</h2>
        <div className="settings-row">
          <label className="settings-label" htmlFor="ui-scale">
            Size
          </label>
          <input
            id="ui-scale"
            className="settings-slider"
            type="range"
            min={SCALE_MIN}
            max={SCALE_MAX}
            step={SCALE_STEP}
            value={settings.uiScale}
            onChange={(e) => setScale(Number(e.target.value))}
          />
          <span className="settings-value">
            {Math.round(settings.uiScale * 100)}%
          </span>
        </div>
        <p className="settings-hint">
          Scales the whole panel (header, chips, timeline, expansion) and
          resizes the panel window proportionally. 100% is the most
          compact size.
        </p>

        <div className="settings-row">
          <label className="settings-label" htmlFor="diff-font">
            Font
          </label>
          <input
            id="diff-font"
            className="settings-input"
            type="text"
            list="diff-font-presets"
            placeholder="Built-in monospace"
            value={settings.diffFontFamily ?? ""}
            onChange={(e) => setFont(e.target.value)}
          />
          <datalist id="diff-font-presets">
            {FONT_PRESETS.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
        </div>
        <p className="settings-hint">
          Diff view font. Empty = built-in monospace stack. Any installed
          font is fine — proportional fonts render but the gutter and
          line-number alignment look ragged, so monospace is recommended.
        </p>
      </section>

      <section className="settings-section">
        <h2 className="settings-section-title">Shortcut</h2>
        <div className="settings-row">
          <label className="settings-label">Panel hotkey</label>
          <button
            type="button"
            className={"settings-hotkey" + (recording ? " recording" : "")}
            onClick={() => {
              if (!recording) {
                setRecording(true);
                setHotkeyError(null);
              }
            }}
          >
            {recording
              ? "Press a shortcut… (Esc to cancel)"
              : settings.panelHotkey}
          </button>
        </div>
        {hotkeyError && <p className="settings-error">{hotkeyError}</p>}
        <p className="settings-hint">
          Required: at least one modifier (Ctrl / Alt / Shift) + a key.
          OS-reserved combos (Alt+Tab, Win+L, etc.) cannot be captured
          and won't react.
        </p>
      </section>

      {settings.updaterAvailable && (
        <section className="settings-section">
          <h2 className="settings-section-title">Updates</h2>
          <div className="settings-radio-group" role="radiogroup">
            {(
              [
                {
                  value: "enabled",
                  label: "Automatic",
                  hint: "Check on startup + every 24h. Tray dot when one's ready.",
                },
                {
                  value: "manual",
                  label: "Manual only",
                  hint: 'No background checks; use the tray "Check for updates" entry.',
                },
                {
                  value: "disabled",
                  label: "Off",
                  hint: 'Updater fully disabled. Tray hides the "Check for updates" entry.',
                },
              ] as const
            ).map((opt) => (
              <label key={opt.value} className="settings-radio">
                <input
                  type="radio"
                  name="update-check"
                  value={opt.value}
                  checked={settings.updateCheck === opt.value}
                  onChange={() => setUpdateMode(opt.value)}
                />
                <span className="settings-radio-label">{opt.label}</span>
                <span className="settings-radio-hint">{opt.hint}</span>
              </label>
            ))}
          </div>
        </section>
      )}

      <section className="settings-section">
        <h2 className="settings-section-title">Auto-fetch</h2>
        <label className="settings-radio">
          <input
            type="checkbox"
            checked={settings.autoFetchOnShow}
            onChange={(e) => setAutoFetch(e.target.checked)}
          />
          <span className="settings-radio-label">
            Fetch on panel open <span className="settings-radio-default">(on by default)</span>
          </span>
          <span className="settings-radio-hint">
            When viewing a single repo, run a quiet background{" "}
            <code>git fetch origin</code> as the panel opens, so a teammate's
            just-pushed commit shows up. Only updates the remote-tracking
            mirror (<code>refs/remotes/origin/*</code>) — never your local
            branches, tags, or working tree. Never blocks the panel, stays
            silent if it needs a password or has no network, and skips repos
            fetched in the last few minutes. The all-repos view never fetches.
            (gitwink still never merges, pushes, or rewrites your work.)
          </span>
        </label>
      </section>

      <footer className="settings-footer">
        <button
          type="button"
          className="settings-link"
          onClick={openSettingsFile}
        >
          Open settings.json
        </button>
        <span className="settings-footer-hint">
          Reveals the raw config in your default editor. Most knobs above
          are mirrored here — auto-managed fields (window positions, repo
          state) shouldn't need hand-edits.
        </span>
      </footer>
    </div>
  );
}
