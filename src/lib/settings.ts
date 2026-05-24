import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";

/** Self-update behaviour. Mirrors Rust `UpdateCheckMode` (serialized
 *  lowercase) — "enabled" auto-checks every 24h, "manual" only on tray
 *  click, "disabled" turns the updater off entirely (no tray dot, no
 *  "Check for updates" item). */
export type UpdateCheckMode = "enabled" | "manual" | "disabled";

/** The user-facing settings slice — mirrors the Rust `AppSettings`. */
export interface AppSettings {
  uiScale: number;
  diffFontFamily: string | null;
  panelHotkey: string;
  panelPinned: boolean;
  updateCheck: UpdateCheckMode;
  /** False for Scoop / Microsoft Store installs — the Updates section
   *  of the Settings window hides because those channels manage their
   *  own updates. */
  updaterAvailable: boolean;
}

/** Built-in diff/code monospace stack — the fallback when no font is
 *  picked. Kept in sync with the `.sbs` rule in styles.css. */
export const MONO_STACK =
  'ui-monospace, SFMono-Regular, "Cascadia Mono", Menlo, monospace';

export const DEFAULT_SETTINGS: AppSettings = {
  uiScale: 1,
  diffFontFamily: null,
  panelHotkey: "CmdOrCtrl+Shift+G",
  panelPinned: false,
  updateCheck: "enabled",
  updaterAvailable: true,
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
 *  current --ui-scale and round to an integer.
 *
 *  Invariant (in practice, not via a shared CSS var):
 *  the JS-owned virtual-row box (set inline as `height: rowHeight` on
 *  `.chip-vrow`) is sized from this integer, and the chip content
 *  inside the row scales via `calc(... * var(--ui-scale))`. There is
 *  no `--chip-row-h` CSS var; descendants don't read the row height.
 *  An `overflow: hidden` guard on `.chip-vrow` keeps a future
 *  larger-than-expected child (longer label, bigger font metric)
 *  from leaking visually into the next virtual row. */
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
 *  Persisting to disk is a separate (debounced) command call.
 *
 *  Order matters: we synchronously push the new snapshot into the
 *  backend in-memory cache (`set_live_settings`) BEFORE emitting, so
 *  any window that mounts during the debounce window calls
 *  `get_settings` and gets the fresh value. Without this, the new
 *  window would read stale disk and disagree with the panel until the
 *  next edit (GPT Pro review D1).
 *
 *  The await is fast (no disk I/O), but we still let it run before the
 *  event emit so the cache update happens-before any other window's
 *  reactive read. */
export async function broadcastSettings(s: AppSettings): Promise<void> {
  setLocal(s);
  try {
    await invoke("set_live_settings", { next: s });
  } catch {
    // Best-effort — if the IPC fails the in-memory copy on this side
    // is still updated, and the event still goes out below.
  }
  void emit(SETTINGS_EVENT, s);
}

/** Load settings from the backend, apply them, and start listening for
 *  live changes from the Settings window. Called once per window mount,
 *  before first render.
 *
 *  The listener is registered BEFORE the initial `get_settings` await
 *  (and we track whether a broadcast already landed) so the
 *  "settings broadcast arrives during initial load" race can't leave
 *  this window stuck on the stale disk snapshot. The freshest write
 *  wins. */
export async function initSettings(): Promise<void> {
  let sawBroadcast = false;
  void listen<AppSettings>(SETTINGS_EVENT, (e) => {
    if (e.payload) {
      sawBroadcast = true;
      setLocal(e.payload);
    }
  });
  try {
    const loaded = await invoke<AppSettings>("get_settings");
    if (!sawBroadcast) setLocal(loaded);
  } catch {
    if (!sawBroadcast) setLocal({ ...DEFAULT_SETTINGS });
  }
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

/** React hook — whether the panel is in pinned mode, re-rendering on
 *  every live change so the pin button glyph flips immediately. */
export function usePanelPinned(): boolean {
  return useSyncExternalStore(subscribe, () => current.panelPinned);
}
