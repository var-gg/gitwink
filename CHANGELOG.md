# Changelog

All notable changes to gitwink will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] — 2026-05-22

### Changed

- The commit timeline is rebuilt on a windowed-pull architecture: it
  pages commits in from the local cache instead of holding the whole
  history in memory, so the all-repos timeline stays responsive no
  matter how many repositories, commits, or branches you have. The
  single-repo DAG view is virtualized the same way.
- The all-repos timeline scrollbar now spans your entire history — drag
  the thumb to jump to any point in time, not only the loaded range.
- The Repo, Author, and Branch filter dropdowns are virtualized, so they
  open instantly even with thousands of entries.

### Added

- A "↑ N new commits" pill appears when the background scanner finds new
  commits while you are scrolled away from the top; click it to jump up
  to them.

### Fixed

- The Repo chip now clearly distinguishes the single repository you have
  drilled into from repositories merely checked in the multi-select
  filter — the two were near-identical shades of blue before.
- The diff window opens at a modest default size instead of reopening
  oversized on high-DPI displays.

## [0.2.2] — 2026-05-21

### Added

- Multi-select repo filter — checkboxes in the Repo chip filter the
  timeline to several repositories at once, alongside the existing
  single-repo mode. Selected repos float to the top of the dropdown when
  it reopens; in single-repo mode the checkboxes are read-only.
  Contributed by @mangchhe (#1).

### Fixed

- Chip dropdowns no longer clip past the edge of the panel — they are
  now clamped to stay within the window.

## [0.1.0] — 2026-05-19

First usable release. Tray-resident, read-only git glance tool.

### Added

- System tray icon with click-to-toggle panel, right-click Quit + Reset
  panel position.
- Panel is borderless, drag-anywhere (native OS drag + manual fallback for
  RDP / Chrome Remote Desktop), position persists across opens, auto-hides
  on focus loss with debounce so OS drags / context menus don't dismiss it.
- First-run repo discovery (Windows: `%USERPROFILE%\{source,Documents,
  Projects,Code,Dev,repos,Desktop}` + every non-system drive root; macOS:
  `~/{Projects,Code,Documents,Developer}`). Honors hard-excludes
  (`node_modules`, `target`, `dist`, `.cache`, `vendor`, `.git`), depth-8,
  stops descending at `.git`. Parallel `ignore`-based walker.
- Unified commit timeline across all repos. Streams in incrementally as
  repos are discovered (no "Loading commits…" wait on cold cache). Walks
  every local branch (not just HEAD) so feature-branch commits show up.
- Filter chips: Repo (search + pinned section), Time range
  (24h / 3d / 7d / 30d / All), Authors (multi-select, counts, recent-first).
- Per-commit markers — `●` / `◆` / `★`. Branch label badges for
  non-current-branch commits.
- Single-repo mode with custom SVG DAG lane drawer, eight-colour palette
  hashed from branch name, main / master / develop / dev / trunk neutral.
- Inline expansion: commit message body + changed-file list with
  NEW/MOD/REN/DEL badges, +/− line counts. GitLens-style filename
  emphasis (basename bold, directory shrink-to-fit).
- Separate diff window (singleton, reused on subsequent opens). Side-by-side
  text diff with horizontally synchronised columns + sticky line numbers.
  PNG / JPG / GIF / WebP / SVG image preview. Local Git LFS object lookup.
  Position, size, and maximised state persist; default opening size is
  ~70% of the primary monitor (clamped). Esc hides for reuse.
- "Copy as AI context" — `c` key or button — emits a markdown block (commit
  meta + changed files + small-enough diff + body) for pasting into AI
  agent chats.
- SQLite cache for repos + commits (paint instantly on warm start);
  diff blob cache with LRU GC at 500 MB.
- 15 unit tests for repo discovery, git read paths, branch labelling,
  and merge / dedupe behaviour.

### Non-goals (deferred)

- No commit / push / merge / fetch UI. Read-only by design.
- No network operations in v0.1. No remote fetch, no LFS download.
- No global hotkey binding (plugin wired, key off by default).
- No installer / code signing — `cargo tauri build` artefacts only.
- No telemetry, no analytics, no phone-home.
