# Changelog

All notable changes to gitwink will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] — 2026-06-10

### Added

- Per-repo identity dots in the all-repos timeline — each repo gets a
  stable color from the same palette the branch lanes use, so interleaved
  commits group visually by repo instead of having to read every name.
- "Copied as AI context ✓" toast — the `c` shortcut now confirms (or
  reports failure) even when the row isn't expanded. Previously the
  keyboard path copied in total silence, and a failure left your previous
  clipboard contents in place with no hint.
- Filter-aware empty states — instead of a bare "No commits match.", the
  panel now names the filter that's hiding the commits ("No commits in
  the last 7 days. Filtered to 2 branches.") and offers one-click outs:
  Show all time, Clear author filter, and All branches in single-repo mode.

### Fixed

- **The timeline no longer shows history your repos don't contain.** When
  an agent (or you) amends or rebases, the rewritten-away commits are now
  reconciled out of the cache — previously they lingered forever next to
  their replacements, which is the one lie a sanity-check glance must
  never tell. Mid-scroll views update in place without losing your
  position, and agent bursts are no longer truncated (per-refresh caps
  raised from 10 to 100 commits per repo).
- Hiding a repo now actually removes its commits from the timeline and
  stops watching its `.git` — previously the rows stayed forever and new
  commits in the hidden repo kept re-appearing.
- Commits made on a detached HEAD (agents checking out a SHA, bisect-style
  loops) and commits reachable only from a tag now show up in the timeline.
- Repos found by the background discovery scan are file-watched
  immediately — on a fresh install, live updates used to start only after
  the next app launch.
- settings.json can no longer be silently wiped: concurrent writers are
  serialized, writes are atomic (temp file + fsync + rename), and a
  hand-edit typo now preserves your original as `settings.json.bak`
  instead of resetting everything to defaults on the next auto-save.
- Diff window position is restored in physical pixels — on scaled
  monitors it used to creep toward the bottom-right on every reopen and
  eventually open off-screen.
- Release-candidate tags now publish as GitHub prereleases, so an rc can
  never reach the in-app updater's "latest" endpoint.

### Changed

- Idle footprint drops sharply in agent-heavy environments: the file
  watcher now reacts only to actual ref movement — `git status` index
  churn and object writes during fetch/gc no longer trigger refreshes —
  hidden-panel refreshes only fire when rows actually changed, and the
  SQLite cache runs in WAL mode with migrations applied once per launch
  instead of replayed on every query.
- Panel summon no longer serializes up to 5,000 commit rows over IPC just
  to discard them — the refill reports a count and the windowed timeline
  pulls rows from the cache.

## [0.5.0] — 2026-06-02

### Added

- Diff window — whole-file view. A `±3 / ±25 / Full` context toggle in the
  diff header expands the view beyond the changed hunks; "Full" shows the
  entire file with additions/deletions still highlighted, right in the
  side-by-side view (no separate editor). Cached only at the default size;
  disabled and server-capped for very large files.
- Diff window — resizable columns. Drag the divider between the old and new
  columns to rebalance them when one side's lines are clipped, double-click
  to reset to 50/50. The split is remembered across diff windows.

### Fixed

- The timeline no longer snaps the open commit, selection, and scroll back
  to the first row a second or two after you click a row. A background
  refresh (panel re-summon, a finished scan, a file-watcher event) is now a
  quiet in-place refresh that keeps your view and follows the commit you
  opened by identity instead of re-anchoring to the top.
- Diff resizer is smooth and robust — the diff no longer re-parses on every
  drag frame, the drag recovers from pointer-cancel / lost-capture, and a
  zero-width edge case can no longer produce invalid layout.
- Hardened the windowed all-repos timeline against several races surfaced by
  a multi-pass review: a stale refresh clobbering a newer filter change,
  opposite-direction page extends splicing a non-contiguous window, and
  recovery anchors matching the wrong row when two repos share a commit hash.

## [0.4.0] — 2026-05-24

### Added

- Settings window — a dedicated panel for adjusting gitwink's behaviour
  and appearance. Open it from the tray menu or the panel header's
  right-click → Settings entry.
- UI scale (size) control in Settings — the panel chrome, chip
  dropdowns, row heights, and inline commit expansions all scale in
  lockstep so the layout never breaks at large sizes.
- Font picker in Settings → Appearance — switch between system fonts
  for commit text without editing settings.json by hand.
- Hotkey recorder for the global panel shortcut — capture a new key
  combination by pressing it, instead of typing the Tauri shortcut spec.
- Updates section in Settings — see your installer channel (Direct /
  Scoop / Microsoft Store), the current update-check mode, and trigger
  a manual update check. A footer link opens settings.json in your
  editor.
- Pin mode for the panel — toggle between glance mode (auto-dismiss on
  blur) and pinned mode (stays open) for keeping the timeline visible
  while you work elsewhere.

### Fixed

- Microsoft Store tile assets are now fully opaque, with a filled white
  wink on a solid purple background. The previous assets rendered the
  wink at alpha 1–33, which Windows alpha-compositing turned into
  chrome-colour holes — the Store certification team read them as
  broken placeholders.
- Diff window — row tint now extends across the full horizontal scroll
  width, the sticky line-number gutter is opaque so long lines don't
  bleed through under horizontal scroll, the right-click menu is scoped
  to the diff content, and the window build is main-thread plus
  single-flight to eliminate a blank-webview race.
- Panel and Settings stability — concurrent Settings opens are
  serialized so they can't race-crash, the Settings window is hidden on
  close instead of destroyed (no async-destroy race), an in-memory
  settings cache prevents stale reads during debounce, and the panel
  blur-dismiss rule consolidates and re-evaluates on child-window
  close. Plus five small correctness fixes from a GPT Pro review of the
  panel and updater paths.

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
