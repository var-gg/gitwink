import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useLingui } from "@lingui/react";

import {
  broadcastSettings,
  useSettingsSnapshot,
  type UpdateCheckMode,
} from "../lib/settings";
import type { AppLocale, LocalePreference } from "../i18n/runtime";
import { settingsMessages as m } from "../i18n/messages/settings";
import { updateProviderLabel } from "../i18n/messages/updates";
import { translate } from "../i18n/translate";
import {
  setPanelSizePreset,
  type PanelSizePreset,
} from "../lib/ipc";
import { ExcludedReposSection } from "./settings/ExcludedReposSection";

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
  const { i18n } = useLingui();
  // Live store subscription — NOT a mount-time copy. This window is hidden
  // (not closed) on X, so a frozen snapshot would go stale the moment any
  // other window changes a setting (📌 pin), and the next edit here would
  // broadcast the stale full object, reverting that change everywhere.
  // broadcastSettings updates the store synchronously, so edits round-trip
  // through the same subscription with no fighting.
  const settings = useSettingsSnapshot();
  const scaleTimer = useRef<number | undefined>(undefined);
  const fontTimer = useRef<number | undefined>(undefined);

  const [recording, setRecording] = useState(false);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [bindingHotkey, setBindingHotkey] = useState(false);
  const [languageBusy, setLanguageBusy] = useState(false);
  const [languageError, setLanguageError] = useState<string | null>(null);
  const [panelLayoutBusy, setPanelLayoutBusy] = useState(false);
  const [panelLayoutError, setPanelLayoutError] = useState<string | null>(null);
  const panelLayoutInFlight = useRef(false);
  const hotkeyBinding = useRef(false);
  // Mirror settings into a ref so the recording effect doesn't have to
  // re-subscribe to keydown whenever an unrelated setting changes.
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  function setScale(uiScale: number) {
    const next = { ...settings, uiScale };
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
    broadcastSettings(next);
    window.clearTimeout(fontTimer.current);
    fontTimer.current = window.setTimeout(() => {
      void invoke("set_diff_font", { family: fam });
    }, PERSIST_DELAY_MS);
  }

  function setUpdateMode(mode: UpdateCheckMode) {
    const next = { ...settings, updateCheck: mode };
    broadcastSettings(next);
    // Persist immediately — radio clicks are single events, not a sweep,
    // so debounce buys nothing and the user expects the tray dot /
    // "Check for updates" item to react right away.
    void invoke("set_update_check", { mode });
  }

  async function setLanguage(preference: LocalePreference) {
    if (languageBusy || preference === settings.localePreference) return;
    setLanguageBusy(true);
    setLanguageError(null);
    try {
      const resolvedLocale = await invoke<AppLocale>("set_locale", {
        preference,
      });
      await broadcastSettings({
        ...settingsRef.current,
        localePreference: preference,
        resolvedLocale,
      });
    } catch (error) {
      setLanguageError(String(error));
    } finally {
      setLanguageBusy(false);
    }
  }

  function setAutoFetch(enabled: boolean) {
    const next = { ...settings, autoFetchOnShow: enabled };
    broadcastSettings(next);
    // A checkbox is a single discrete event — persist immediately, no debounce.
    void invoke("set_auto_fetch_on_show", { enabled });
  }

  async function applyPanelLayout(preset: PanelSizePreset) {
    if (panelLayoutInFlight.current) return;
    panelLayoutInFlight.current = true;
    setPanelLayoutBusy(true);
    setPanelLayoutError(null);
    try {
      await setPanelSizePreset(preset);
    } catch (error) {
      setPanelLayoutError(String(error));
    } finally {
      panelLayoutInFlight.current = false;
      setPanelLayoutBusy(false);
    }
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
      if (document.visibilityState === "hidden") {
        flushPending();
        setRecording(false);
        setHotkeyError(null);
        setLanguageError(null);
        setPanelLayoutError(null);
      }
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
          i18n._(m.hotkeyModifierError),
        );
        return;
      }
      if (hotkeyBinding.current) return;
      hotkeyBinding.current = true;
      setBindingHotkey(true);
      void (async () => {
        try {
          await invoke("set_panel_hotkey", { spec: accel });
          const next = { ...settingsRef.current, panelHotkey: accel };
          broadcastSettings(next);
          setRecording(false);
          setHotkeyError(null);
        } catch (err) {
          // Most common case: the combo is already held by another app
          // (Windows registers globally, first-bind wins). Surface inline.
          setHotkeyError(String(err));
        } finally {
          hotkeyBinding.current = false;
          setBindingHotkey(false);
        }
      })();
    }
    document.addEventListener("keydown", onKeyDown, { capture: true });
    return () =>
      document.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [recording, i18n]);

  const updateSummary = translate(
    i18n,
    settings.updatePolicy.installAction.kind === "externalCommand"
      ? m.policyExternal
      : m.policyInApp,
    {
      checkProvider: updateProviderLabel(
        i18n,
        settings.updatePolicy.checkProvider,
      ),
      installProvider: updateProviderLabel(
        i18n,
        settings.updatePolicy.installAction.provider,
      ),
    },
  );
  const automaticHint = translate(i18n, m.automaticHint, {
    provider: updateProviderLabel(i18n, settings.updatePolicy.checkProvider),
  });

  return (
    <div className="settings">
      <h1 className="settings-title">{i18n._(m.title)}</h1>

      <section className="settings-section">
        <h2 className="settings-section-title">{i18n._(m.languageTitle)}</h2>
        <div className="settings-row">
          <label className="settings-label" htmlFor="app-language">
            {i18n._(m.languageLabel)}
          </label>
          <select
            id="app-language"
            className="settings-input"
            value={settings.localePreference}
            disabled={languageBusy}
            aria-describedby={languageError ? "language-error" : "language-help"}
            onChange={(event) =>
              void setLanguage(event.target.value as LocalePreference)
            }
          >
            <option value="system">{i18n._(m.languageSystem)}</option>
            <option value="en">{i18n._(m.languageEnglish)}</option>
            <option value="ko">{i18n._(m.languageKorean)}</option>
            <option value="ja">{i18n._(m.languageJapanese)}</option>
          </select>
        </div>
        {languageError && (
          <p id="language-error" className="settings-error" role="alert">
            {languageError}
          </p>
        )}
        <p id="language-help" className="settings-hint">
          {i18n._(m.languageHint)}
        </p>
      </section>

      <section className="settings-section">
        <h2 className="settings-section-title">{i18n._(m.appearanceTitle)}</h2>
        <div className="settings-row">
          <label className="settings-label" htmlFor="ui-scale">
            {i18n._(m.sizeLabel)}
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
          {i18n._(m.sizeHint)}
        </p>

        <div className="settings-row settings-panel-layout-row">
          <span className="settings-label">{i18n._(m.panelSizeLabel)}</span>
          <div className="settings-button-group" role="group">
            <button
              type="button"
              className="settings-compact-button"
              disabled={panelLayoutBusy}
              onClick={() => void applyPanelLayout("wide")}
            >
              {i18n._(m.panelWide)}
            </button>
            <button
              type="button"
              className="settings-compact-button"
              disabled={panelLayoutBusy}
              onClick={() => void applyPanelLayout("default")}
            >
              {i18n._(m.panelReset)}
            </button>
          </div>
        </div>
        {panelLayoutError && (
          <p className="settings-error" role="alert">
            {panelLayoutError}
          </p>
        )}
        <p className="settings-hint">{i18n._(m.panelSizeHint)}</p>

        <div className="settings-row">
          <label className="settings-label" htmlFor="diff-font">
            {i18n._(m.fontLabel)}
          </label>
          <input
            id="diff-font"
            className="settings-input"
            type="text"
            list="diff-font-presets"
            placeholder={i18n._(m.fontPlaceholder)}
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
          {i18n._(m.fontHint)}
        </p>
      </section>

      <section className="settings-section">
        <h2 className="settings-section-title">{i18n._(m.shortcutTitle)}</h2>
        <div className="settings-row">
          <label className="settings-label">{i18n._(m.hotkeyLabel)}</label>
          <button
            type="button"
            className={"settings-hotkey" + (recording ? " recording" : "")}
            aria-pressed={recording}
            aria-busy={bindingHotkey}
            aria-describedby={hotkeyError ? "hotkey-error" : "hotkey-help"}
            onClick={() => {
              if (!recording) {
                setRecording(true);
                setHotkeyError(null);
              }
            }}
          >
            {recording
              ? i18n._(m.hotkeyRecording)
              : settings.panelHotkey}
          </button>
        </div>
        {hotkeyError && (
          <p id="hotkey-error" className="settings-error" role="alert">
            {hotkeyError}
          </p>
        )}
        <p id="hotkey-help" className="settings-hint">
          {i18n._(m.hotkeyHelp)}
        </p>
      </section>

      <ExcludedReposSection />

      <section className="settings-section">
        <h2 className="settings-section-title">{i18n._(m.updatesTitle)}</h2>
        <p className="settings-hint">
          {updateSummary}
        </p>
        {settings.updatePolicy.supportsBackgroundChecks ? (
          <div className="settings-radio-group" role="radiogroup">
            {(
              [
                {
                  value: "enabled",
                  label: i18n._(m.updatesAutomatic),
                  hint: automaticHint,
                },
                {
                  value: "manual",
                  label: i18n._(m.updatesManual),
                  hint: i18n._(m.updatesManualHint),
                },
                {
                  value: "disabled",
                  label: i18n._(m.updatesOff),
                  hint: i18n._(m.updatesOffHint),
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
        ) : (
          <p className="settings-hint">
            {i18n._(m.updatesUnsupported)}
          </p>
        )}
      </section>

      <section className="settings-section">
        <h2 className="settings-section-title">{i18n._(m.autoFetchTitle)}</h2>
        <label className="settings-radio">
          <input
            type="checkbox"
            checked={settings.autoFetchOnShow}
            onChange={(e) => setAutoFetch(e.target.checked)}
          />
          <span className="settings-radio-label">
            {i18n._(m.autoFetchLabel)}{" "}
            <span className="settings-radio-default">{i18n._(m.defaultOn)}</span>
          </span>
          <span className="settings-radio-hint">
            {i18n._(m.autoFetchHint)}
          </span>
        </label>
      </section>

      <footer className="settings-footer">
        <button
          type="button"
          className="settings-link"
          onClick={openSettingsFile}
        >
          {i18n._(m.openSettingsFile)}
        </button>
        <span className="settings-footer-hint">
          {i18n._(m.openSettingsFileHint)}
        </span>
      </footer>
    </div>
  );
}
