import { msg } from "@lingui/core/macro";

/** Semantic IDs for the settings domain. Keeping descriptors beside the
 * domain (instead of one app-wide message dump) makes ownership and review
 * boundaries explicit while Lingui still extracts a single locale catalog. */
export const settingsMessages = {
  title: msg({ id: "settings.title", message: "Settings" }),
  languageTitle: msg({ id: "settings.language.title", message: "Language" }),
  languageLabel: msg({ id: "settings.language.label", message: "App language" }),
  languageSystem: msg({ id: "settings.language.system", message: "Follow system" }),
  languageEnglish: msg({ id: "settings.language.english", message: "English" }),
  languageKorean: msg({ id: "settings.language.korean", message: "Korean" }),
  languageJapanese: msg({ id: "settings.language.japanese", message: "Japanese" }),
  languageHint: msg({
    id: "settings.language.hint",
    message:
      "Applies immediately to every gitwink window and the tray. Unsupported system languages use English.",
  }),
  appearanceTitle: msg({ id: "settings.appearance.title", message: "Appearance" }),
  sizeLabel: msg({ id: "settings.appearance.size", message: "UI scale" }),
  sizeHint: msg({
    id: "settings.appearance.sizeHint",
    message:
      "Controls readability independently from information space. The normal panel follows this scale while preserving your chosen workspace size.",
  }),
  panelSizeLabel: msg({
    id: "settings.appearance.panelSize",
    message: "Panel size",
  }),
  panelWide: msg({
    id: "settings.appearance.panelWide",
    message: "Wide",
  }),
  panelReset: msg({
    id: "settings.appearance.panelReset",
    message: "Reset layout",
  }),
  panelSizeHint: msg({
    id: "settings.appearance.panelSizeHint",
    message:
      "Resize any edge for an exact size, choose Wide for file paths, or reset size and placement. Maximizing remains a temporary review mode.",
  }),
  fontLabel: msg({ id: "settings.appearance.font", message: "Font" }),
  fontPlaceholder: msg({
    id: "settings.appearance.fontPlaceholder",
    message: "Built-in monospace",
  }),
  fontHint: msg({
    id: "settings.appearance.fontHint",
    message:
      "Diff view font. Empty uses the built-in monospace stack. Any installed font works, but monospace is recommended for aligned gutters and line numbers.",
  }),
  shortcutTitle: msg({ id: "settings.shortcut.title", message: "Shortcut" }),
  hotkeyLabel: msg({ id: "settings.shortcut.panelHotkey", message: "Panel hotkey" }),
  hotkeyRecording: msg({
    id: "settings.shortcut.recording",
    message: "Press a shortcut… (Esc to cancel)",
  }),
  hotkeyModifierError: msg({
    id: "settings.shortcut.modifierError",
    message: "Need at least one modifier (Ctrl / Alt / Shift) + a key.",
  }),
  hotkeyHelp: msg({
    id: "settings.shortcut.help",
    message:
      "Required: at least one modifier (Ctrl / Alt / Shift) + a key. OS-reserved combos (Alt+Tab, Win+L, etc.) cannot be captured and won't react.",
  }),
  updatesTitle: msg({ id: "settings.updates.title", message: "Updates" }),
  updatesAutomatic: msg({
    id: "settings.updates.automatic",
    message: "Automatic checks",
  }),
  updatesManual: msg({ id: "settings.updates.manual", message: "Manual only" }),
  updatesManualHint: msg({
    id: "settings.updates.manualHint",
    message: "No background checks; use the tray “Check for updates” entry.",
  }),
  updatesOff: msg({ id: "settings.updates.off", message: "Off" }),
  updatesOffHint: msg({
    id: "settings.updates.offHint",
    message: "Updater fully disabled. The tray hides “Check for updates.”",
  }),
  updatesUnsupported: msg({
    id: "settings.updates.unsupported",
    message: "This installation does not support checks inside gitwink.",
  }),
  autoFetchTitle: msg({ id: "settings.autoFetch.title", message: "Auto-fetch" }),
  autoFetchLabel: msg({
    id: "settings.autoFetch.label",
    message: "Fetch on panel open",
  }),
  defaultOn: msg({ id: "settings.defaultOn", message: "(on by default)" }),
  autoFetchHint: msg({
    id: "settings.autoFetch.hint",
    message:
      "When viewing one repository, gitwink quietly runs git fetch origin as the panel opens so a teammate's newly pushed commit appears. It only updates the remote-tracking mirror—never local branches, tags, or the working tree. It never blocks the panel, stays silent when authentication or the network is unavailable, and skips recently fetched repositories. The all-repositories view never fetches. gitwink never merges, pushes, or rewrites your work.",
  }),
  openSettingsFile: msg({
    id: "settings.openFile.action",
    message: "Open settings.json",
  }),
  openSettingsFileHint: msg({
    id: "settings.openFile.hint",
    message:
      "Opens the raw configuration in your default editor. Most options above are mirrored here; auto-managed window positions and repository state should not need hand edits.",
  }),
  policyExternal: msg({
    id: "settings.updates.policyExternal",
    message: "Checks use {checkProvider}. {installProvider} owns installation.",
  }),
  policyInApp: msg({
    id: "settings.updates.policyInApp",
    message: "Checks use {checkProvider}. Updates install in-app through {installProvider}.",
  }),
  automaticHint: msg({
    id: "settings.updates.automaticHint",
    message:
      "Check {provider} at startup and every 24 hours. A tray dot appears when an update is ready.",
  }),
} as const;
