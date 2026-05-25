# gitwink

**English** · [한국어](README.ko.md) · [日本語](README.ja.md)

[![Release](https://img.shields.io/github/v/release/var-gg/gitwink)](https://github.com/var-gg/gitwink/releases/latest)
[![Microsoft Store](https://img.shields.io/badge/Microsoft%20Store-Available-0078D4?logo=microsoftstore&logoColor=white)](https://apps.microsoft.com/detail/9P0S21GJD53F)

> Tray-resident, read-only git glance for the AI-agent era.

**Status:** v0.4 — usable. Cold-start friendly tray app.

![gitwink](docs/images/hero.gif)

gitwink lives in your system tray. Click it to glance at recent commit
activity across **all** your local repos. It is **not** a git client — it
cannot commit, push, merge, or modify anything. Read-only by design.

## Download

**Windows — [Microsoft Store](https://apps.microsoft.com/detail/9P0S21GJD53F):**

[**Get gitwink on the Microsoft Store →**](https://apps.microsoft.com/detail/9P0S21GJD53F)

The Store build is signed by Microsoft during certification, so no
SmartScreen prompt appears. The Store also owns updates — gitwink's
in-app updater stays out of the way for this channel.

**Windows — [Scoop](https://scoop.sh):**

```sh
scoop bucket add var-gg https://github.com/var-gg/scoop-bucket
scoop install gitwink
```

Update later with `scoop update gitwink`. Scoop installs the build by
extraction, so the SmartScreen prompt never appears.

**Or download directly:**

[**Download the latest release →**](https://github.com/var-gg/gitwink/releases/latest)

- **Windows** — `.exe` (NSIS installer) or `.msi`
- **macOS** — `.dmg` (universal)

Direct downloads are unsigned for now, so Windows SmartScreen / macOS
Gatekeeper will warn on first launch — the release notes have the
bypass steps. Prefer to build it yourself? See [Development](#development).

## Code signing

Different install channels carry their own trust path:

- **Microsoft Store** — packages are automatically re-signed by
  Microsoft during certification; SmartScreen stays silent.
- **Scoop** — installs by extraction, no SmartScreen prompt either.
- **Direct downloads** (`.exe` / `.msi`) — currently unsigned. gitwink
  participates in the [SignPath Foundation](https://signpath.org/) free
  code-signing program for open-source software (see the
  [Code signing policy](CODE_SIGNING_POLICY.md)); the SignPath
  certificate will sign these artefacts once approved.

## Origin

I used to live in VS Code with GitLens pinned. The branch graph,
heat-mapped blame, the lens annotations — that *was* my git workflow.
Then 2025 happened. With Cursor, Claude Code, and Codex doing the
actual editing, the editor itself became optional. The only thing
dragging me back was GitLens.

That felt wasteful — booting an entire IDE just to peek at commit
history. The agent runs the git commands now; I only need to
sanity-check the result, occasionally, when something looks off.
gitwink is the smallest possible tool for *that* loop — a tray icon
that expands into a glance, hands the commit off as AI context, and
gets out of the way.

No commit. No push. No merge. If I need git surgery, I tell the agent.

## The loop

The 0.5-second confirm loop:

```
agent commits  →  tray click  →  inline expand  →  "Copy as AI context"
                                                   →  paste into Claude/Codex
                                                   →  "did the agent do this right?"
```

## Features

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

## Diff window

For the *"wait, did the agent actually do that?"* moments. Click any
commit and a separate window opens — full file sidebar, side-by-side
diff with synchronised scroll, inline image preview for binary assets,
and a singleton that remembers position, size, and maximised state.

![diff window](docs/images/diff.gif)

## Tech

Tauri 2 · Rust · React + TypeScript · `git2` · SQLite · custom SVG DAG
drawer · no telemetry, no phone-home — the only network access is an
opt-out check for updates.

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
