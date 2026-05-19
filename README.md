# gitwink

> Tray-resident, read-only git glance for the AI-agent era.

**Status:** v0.1 — usable. Cold-start friendly tray app.

gitwink lives in your system tray. Click it to glance at recent commit
activity across **all** your local repos. It is **not** a git client — it
cannot commit, push, merge, or modify anything. Read-only by design.

The 0.5-second confirm loop:

```
agent commits  →  tray click  →  inline expand  →  "Copy as AI context"
                                                   →  paste into Claude/Codex
                                                   →  "did the agent do this right?"
```

## What v0.1 ships

- System tray icon (Windows tray / macOS menu bar) with click-to-toggle
  and right-click Reset position / Open settings file / Quit.
- Global hotkey `Ctrl+Shift+G` (Windows) / `Cmd+Shift+G` (macOS) to
  summon or dismiss the panel from anywhere. Change it by editing
  `panel_hotkey` in `settings.json` (right-click the tray icon →
  "Open settings file…") to any Tauri shortcut spec, e.g.
  `"Alt+Space"`, `"Ctrl+Alt+Backquote"`. Restart gitwink to apply.
- First-run discovery walks default user dirs (`source`, `Documents`,
  `Projects`, `Code`, `Dev`, `repos`, `Desktop`, every non-system drive on
  Windows / `~/Projects`, `~/Code`, `~/Documents`, `~/Developer` on macOS).
  Results cached in SQLite at `%APPDATA%\gg.var.gitwink\cache.db`.
- Unified commit timeline across all repos, with chips above for
  filtering: Repo (search + pinning), Time range (24h / 3d / 7d / 30d /
  All), Authors (multi-select with counts).
- Per-row markers — `●` commit · `◆` merge · `★` tagged. Branch label
  badges for commits that aren't on the currently checked-out branch.
- Single-repo mode: pick a repo and the panel switches to a per-branch
  view with a custom SVG DAG lane drawer (eight-colour palette, hashed
  from branch name; main / master / develop neutral).
- Inline expansion on click: commit message body + changed-file list
  with NEW/MOD/REN/DEL badges, `+/−` line counts, `bin` + size for
  binaries, GitLens-style filename emphasis.
- Separate diff window (singleton, reused, position/size + maximised
  persisted) for full reading: file sidebar + side-by-side diff with
  synchronised horizontal scroll. PNG / JPG / GIF / WebP / SVG image
  preview built in (before / after, with checker background). Local
  Git LFS objects are looked up automatically; missing ones are
  explained inline.
- Copy as AI context — `c` key or button — produces a markdown block
  with the commit, file list, and (if small enough) full diff, ready
  to paste into Claude / Codex / Cursor.

## Tech

Tauri 2 · Rust · React + TypeScript · `git2` · SQLite · custom SVG DAG
drawer · no telemetry, no phone-home, no network.

## Development

```bash
pnpm install
pnpm tauri dev
```

Requires: Node 20+, Rust stable (msvc toolchain on Windows), Visual C++
Build Tools (Windows) or Xcode CLT (macOS).

## Platforms

- Windows 10/11 — primary target, tested on dev hardware
- macOS 13+ — should work, less battle-tested
- Linux — later

## License

[MIT](LICENSE)
