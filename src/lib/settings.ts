import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";

/** The user-facing settings slice — mirrors the Rust `AppSettings`. */
export interface AppSettings {
  uiScale: number;
  diffFontFamily: string | null;
  panelHotkey: string;
}

/** Built-in diff/code monospace stack — the fallback when no font is
 *  picked. Kept in sync with the `.sbs` rule in styles.css. */
export const MONO_STACK =
  'ui-monospace, SFMono-Regular, "Cascadia Mono", Menlo, monospace';

export const DEFAULT_SETTINGS: AppSettings = {
  uiScale: 1,
  diffFontFamily: null,
  panelHotkey: "CmdOrCtrl+Shift+G",
};

/** Timeline row height in px at scale 1.0 — the value the fixed-row
 *  virtualization was tuned for. The scaled height is always an integer
 *  so the JS geometry and the CSS `--timeline-row-h` cannot drift apart. */
export const BASE_TIMELINE_ROW_H = 31;

/** The timeline row height for a UI scale — one integer, shared by the
 *  virtualization math (ROW_H) and the `--timeline-row-h` CSS property. */
export function timelineRowH(scale: number): number {
  return Math.round(BASE_TIMELINE_ROW_H * scale);
}

/** Scale any chip-dropdown base px (row height, viewport cap) by the
 *  current --ui-scale and round to an integer. JS uses the result for
 *  virtual-row heights; the chip CSS's content height scales by the
 *  same factor via calc(... * var(--ui-scale)), so the two track. */
export function chipRowH(scale: number, basePx: number): number {
  return Math.round(basePx * scale);
}

/** App-wide event carrying a full settings snapshot — broadcast by the
 *  Settings window so every window re-applies without a disk round-trip. */
const SETTINGS_EVENT = "settings://changed";

let current: AppSettings = { ...DEFAULT_SETTINGS };
const listeners = new Set<() => void>();

/** Mirror live settings into CSS custom properties on :root; styles.css
 *  reads the vars. Every window calls this on load + on live change. */
export function applySettings(s: AppSettings): void {
  const root = document.documentElement;
  root.style.setProperty("--ui-scale", String(s.uiScale));
  root.style.setProperty("--timeline-row-h", `${timelineRowH(s.uiScale)}px`);
  root.style.setProperty(
    "--diff-font-family",
    s.diffFontFamily?.trim() ? s.diffFontFamily : MONO_STACK,
  );
}

function setLocal(s: AppSettings): void {
  current = s;
  applySettings(s);
  listeners.forEach((fn) => fn());
}

/** The settings snapshot last loaded or broadcast into this window. */
export function getCurrentSettings(): AppSettings {
  return current;
}

/** Apply a settings change in this window AND broadcast it to every other
 *  window — the live-preview path the Settings window drives on each edit.
 *  Persisting to disk is a separate (debounced) command call. */
export function broadcastSettings(s: AppSettings): void {
  setLocal(s);
  void emit(SETTINGS_EVENT, s);
}

/** Load settings from the backend, apply them, and start listening for
 *  live changes from the Settings window. Called once per window mount,
 *  before first render. */
export async function initSettings(): Promise<void> {
  try {
    setLocal(await invoke<AppSettings>("get_settings"));
  } catch {
    setLocal({ ...DEFAULT_SETTINGS });
  }
  void listen<AppSettings>(SETTINGS_EVENT, (e) => {
    if (e.payload) setLocal(e.payload);
  });
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** React hook — the current UI scale, re-rendering on every live change. */
export function useUiScale(): number {
  return useSyncExternalStore(subscribe, () => current.uiScale);
}
