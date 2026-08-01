# Changelog

All notable changes to gitwink will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.1] — 2026-08-01

A lifecycle reliability patch for long-lived tray sessions. Origin refreshes
now complete through one coordinated flight, and repositories created after
startup join the catalog when the panel is summoned.

### Fixed

- **New repositories no longer disappear during long tray sessions.**
  Summoning the panel now rechecks IDE hints and learned workspace roots, so a
  sibling repository created after startup joins the catalog without restarting
  gitwink. Refreshes run off the UI thread and share a 60-second cooldown plus
  a single-flight lease, preserving the app's no-idle-polling contract.
- **Reliable explicit origin refresh.** Panel-open auto-fetch and the ↻ action
  now share one common-Git-directory flight instead of racing the same remote
  refs. Manual refresh applies one 30-second end-to-end deadline, transient
  external ref locks receive bounded retries, and timeout cleanup terminates
  Git's helper process tree. Flight lifecycle events keep the UI busy until the
  real completion, deterministically refresh commits and refs, expose
  accessible status text, and record only non-secret timing diagnostics.

## [0.12.0] — 2026-07-12

An agent-ready interaction and delivery release. AI tools can land the user on
exact Git context without a vendor-specific bridge, search composes with the
visible timeline scope, the tray panel can grow into a full inspection surface,
and update behavior is explicit for every distribution channel.

### Added

- **A read-only agent control surface.** `gitwinkctl capabilities --json`,
  `show commit`, and `show file-history` let Codex, Claude Code, scripts, and
  other tools open the exact repository context a user should inspect. The
  protocol resolves revisions and subdirectory paths safely, reuses the running
  app, reports structured outcomes, and requires neither MCP nor a
  vendor-specific skill.
- **English, Korean, and Japanese UI.** System-language detection and an
  explicit live preference now cover the panel, diff, Settings, update flows,
  native tray menu, and errors. Unsupported system locales fall back to English
  without shipping separate language-pack installers.
- **A resizable and maximizable panel.** Drag any edge or corner, choose default
  or wide presets, or maximize for deep history reading. gitwink preserves a
  scale-independent normal size, restores it across displays, and follows the
  native maximize conventions of Windows and macOS.
- **Search that composes with timeline filters.** Commit search now remains
  inside the active repository, author, branch, and time scope. Every scope is
  visible and editable in search mode, all-history demand stays indexed and
  cancellable, and jumping to a result round-trips through view history without
  widening the query.
- **Microsoft Store-native in-app updates.** Store installs now ask the
  device's Store catalog—not GitHub—whether a certified package is actually
  available, show the same header/tray indicator as direct installs, and can
  download, install, report progress and restart without sending the user to
  the Store app. Silent delivery is used when Windows permits it; otherwise the
  native consent prompt is attached to the gitwink panel.
- **Browser-free Store listing submissions.** The release workflow validates
  and uploads the reviewed MSIX bundle and ordered locale screenshots through
  Microsoft's packaged-app API. Omitted locales and non-desktop listing assets
  remain untouched, draft ownership is checked before mutation, and failures
  retain a recoverable submission ID instead of depending on browser entry.

### Changed

- **Update delivery is capability-driven.** Store, Scoop, WinGet/direct and
  macOS distributions now share an explicit policy contract that separates
  update discovery from install ownership. Adding a channel no longer requires
  UI code to recognize provider names. GitHub releases remain drafts until CI
  proves that MSI, NSIS and both macOS architectures have valid signed routes.
- **Repository path intake is explicit and resilient.** The footer distinguishes
  native folder browsing from clipboard paste, supports Ctrl+V and Cmd+V from
  the panel, normalizes quoted file-manager paths, accepts repository
  subdirectories, and reports invalid or unavailable clipboard content without
  silently opening the wrong picker.
- **Product documentation now reflects the shipping UI.** The three README
  languages, Store screenshots, agent-control contract, update behavior, panel
  sizing, search scope, file history, and recoverable repository lifecycle now
  describe the same product surface.

### Fixed

- **Manual update checks always finish visibly.** Overlapping automatic and
  manual checks coalesce, Store results are cached within Microsoft's query
  window, and explicit checks now show checking, up-to-date or failure feedback
  even when the panel was hidden. Mandatory Store updates cannot be suppressed
  by stale skip or snooze preferences.
- **Scoop update preferences remain configurable.** Scoop owns installation,
  but users can still select automatic, manual or disabled GitHub checks instead
  of having the entire Updates section hidden.
- **Native controls honor the active theme.** Language and other native select
  menus no longer open as light surfaces with unreadable disabled entries while
  Windows is using dark mode.
- **Summoning surfaces the panel over the active app.** A tray click, global
  shortcut, second launch, or agent handoff now reasserts the intended native
  stacking state before focusing, so a previously pinned panel cannot remain
  hidden behind a browser or video window.

## [0.11.0] — 2026-07-11

A repository-lifecycle and history release. Repositories can now leave and
return without losing user intent, filters describe the scope they actually
query, and complete-history work stays truthful and responsive even when refs,
drives, or views change underneath it.

### Added

- **Recoverable repository removal.** Right-click any repository and choose
  "Remove from list" without touching its folder on disk. The exclusion is
  durable, detaches watchers and cached history safely, and can be restored from
  Settings. If a different repository later occupies the same path, gitwink
  asks before replacing the saved identity instead of silently inheriting its
  preferences. Contributed by @jhkhub (#3, #4). The shipping implementation was
  comprehensively redesigned around a recoverable exclusion lifecycle.
- **Context-aware Repo and Author filters.** Author choices now reflect the
  selected repositories and time range; repository counts reflect the selected
  authors and time range. A selected value that becomes unavailable remains
  visible as an explicit zero-result filter instead of silently widening the
  query.
- **Complete history on demand.** All-time browsing and commit search can build
  a persistent local history index for only the repositories the current view
  needs. Published snapshots remain usable while a refresh runs, progress and
  partial coverage are visible, and cancelled or changing scans never publish a
  mixed result.

### Changed

- **The panel header now has two stable rows.** Window chrome, navigation,
  search, pin and close controls stay separate from the contextual Repo, Branch,
  Time and Author filters. Long names, narrow panels, high UI scale and transient
  status badges no longer make unrelated controls jump or collide.
- **Expensive history work is cancellable and coalesced.** Repeated requests for
  the same file share one worker, replacement requests cancel stale work, and
  complete-history indexing prioritizes the visible demand while keeping
  previously published results readable.
- **Repository state changes are transactional.** Discovery, exclusion,
  restoration, watcher attachment, cache publication and missing-drive recovery
  now share lifecycle fences. Windows path aliases, UNC spellings, linked
  worktrees and rapid remove/restore cycles cannot resurrect stale rows or apply
  preferences to the wrong physical repository.

### Fixed

- **Repository totals stay authoritative.** Re-adding an existing repository no
  longer increments the count, and removing one no longer decrements it twice.
  The catalog itself is now the source of truth rather than event-by-event count
  arithmetic. Fixes #2.
- **Rolling time windows are honest.** Reaching the normal 1/3/7/30-day boundary
  no longer raises a false "Git objects couldn't be read" warning. Rows and
  author counts expire at the exact boundary even while the panel stays open,
  and incomplete or shallow snapshots are retried after the repository recovers.
- **One bad ref no longer hides healthy history.** Readable commits remain
  visible with an explicit partial-coverage warning, destructive cache cleanup
  stays disabled, valid tags that point to non-commit objects are not called
  corruption, and an unrelated bad tag does not contaminate an explicitly
  selected branch.
- **Filter and search scope stays exact.** Opening Authors after narrowing Repos
  no longer shows authors from every repository; facet responses are tied to the
  same immutable cache generation as the visible timeline; and stale async
  searches, branch lists or counts cannot overwrite a newer view.
- **Overlay and keyboard interactions are deterministic.** Dropdowns and context
  menus remain inside the usable panel area, only the topmost layer consumes
  Esc, IME composition cannot trigger underlying shortcuts, and virtualized chip
  lists keep focus and accessible state aligned with what is visible.

## [0.10.1] — 2026-07-04

A hardening release. A full multi-agent QA sweep of v0.10.0 surfaced 29 verified
defects — 13 bugs, 8 performance bottlenecks, 8 rough edges — and every one is
fixed here, each as its own commit and each build- and test-verified. No new
features; the whole release is making what shipped in 0.10.0 solid.

### Fixed

- **Diffs show the right lines again.** A file whose patch contained a line
  starting with `-- ` or `++ ` (an email signature, `--` in prose, a Markdown
  rule) shifted every following line number; and asking for one file's history
  could splice in hunks from similarly-named files, because the path was matched
  as a glob instead of a literal. Both are corrected and now regression-tested.
- **Expanding a commit no longer jerks the timeline.** Opening a commit whose row
  had scrolled to the edge of the virtualized band keeps the scroll position put.
- **Esc does one thing at a time.** With a commit expanded, Esc collapses it
  before it will close the panel; a context menu, an open find bar, and a
  dropdown each take their turn in a stable order instead of racing each other.
- **Korean / CJK input is safe everywhere.** Pressing Enter or Esc mid-composition
  in the search box, the diff find bar, or any dropdown confirms the text instead
  of firing the shortcut underneath it.
- **Opening a repo that can't be read** now says so, instead of leaving the
  previous repo's commits on screen as if they belonged to this one.
- **Warp / jump edge cases.** Jumping to a searched commit clears any file-history
  scope; a jump past the loaded window lands with a visible notice and recovery
  instead of silently at the top; a stale jump target no longer lingers after you
  navigate by hand; and stepping Back across repos restores that view's branch
  filter, not whatever happens to be on disk now.
- **Buttons keep working.** A window-level Enter no longer hijacks a focused panel
  button, and after clicking a chip or a Copy button, `j` / `k` keep navigating.
- **Settings** no longer reverts a freshly-toggled "keep panel pinned" from a
  stale form snapshot.
- **Ctrl+F on an empty diff** no longer arms an invisible find bar that then eats
  the next Esc.

### Performance

- **Big repos stay responsive.** Branch-label computation stops walking up to
  5,000 commits past the visible window; a file's history stops re-scanning the
  full history (50k+ commits) on every summon and cache invalidation; and commits
  with thousands of changed files are cached, their file list capped with an
  honest "showing N of M" footer.
- **The diff window doesn't freeze.** Syntax highlighting upgrades asynchronously
  as you scroll instead of blocking it, and a minified single-line file is capped
  instead of locking the window.
- **Summoning is cheaper.** A single-repo view skips re-shipping multi-megabyte
  commit lists over IPC when nothing changed; a dead or unreachable
  network-drive repo is skipped (with background recovery) instead of blocking
  the whole refresh; and the diff minimap memoizes its segment marks.

### Changed

- **State survives navigation.** The timeline stays mounted across opening and
  closing search (no more full re-scan on every toggle), the diff keeps its
  scroll position and find state across a context-size toggle, relative
  timestamps tick while the panel is open, and view-history stops recording
  no-op navigation steps.
- **A second launch summons the existing window** instead of starting a second
  instance.

## [0.10.0] — 2026-07-02

### Added

- **Find inside a diff (Ctrl+F).** The side-by-side view is virtualized, so the
  browser's own find can't see off-screen lines — this searches the whole diff.
  `Ctrl+F` opens a bar that jumps to each match, counts them (`3/17`), cycles
  with Enter / Shift+Enter, and tints every hit. Esc closes the bar (a second
  Esc closes the window). Works with Korean/CJK input and non-Latin keyboards.
- **Fetch now — an explicit ↻ button** next to the branch chip. It runs the same
  quiet, `origin`-pinned, read-only fetch as auto-fetch, but on demand: it works
  even with auto-fetch turned off, ignores the cooldown, and tells you the
  outcome (spins → ✓, or ! with why), refreshing the ahead/behind counts and the
  "last fetch" age. So there's never any doubt about whether you're looking at
  current remote state.
- **View history — go back and forward (Alt+← / Alt+→).** Every place you jump —
  warping to a searched commit, opening a file's history, switching repos —
  becomes a step you can walk back and forward through, like a browser, with a
  matching `‹ ›` pair in the header. Esc walks the same history. Exiting a file's
  history now returns you to wherever you opened it from (e.g. back to all repos)
  instead of stranding you in one repo.
- **File history without hunting.** Reach a file's history from the right-click
  menu — on a diff-window file, on the diff content, or on a changed-file row in
  the timeline — or from a visible 🕘 on the timeline's changed-file rows, not
  just the diff window.
- **Search names what it actually searched.** A search with no matches that was
  scoped to one repo now says so and offers a one-click "Search all repos,"
  instead of a bare "no matches" that reads like the commit doesn't exist.
- **Copy this file's diff** — a right-click action in the diff window that copies
  the patch you're reading, with a reference header, ready for a chat prompt.
- **A first-run tip** naming the non-obvious moves — search, expand, file
  history, copy-for-AI — shown once and dismissable.

### Changed

- **The diff window is honest when something fails.** A commit whose file list
  can't load says so (instead of a permanent "Loading files…"), and a diff that
  fails to fetch shows a real error instead of a misleading "No textual diff."
- **A filter that's narrowing the view is visible at a glance** — the repo,
  author, and branch chips gain an accent dot when they're not on "all," so a
  near-empty list's cause is obvious.
- **Diff sidebar polish** — status badges (NEW / MOD / DEL / REN) and rename
  arrows now match the panel; the sidebar scales with the UI-scale setting and
  can no longer sprout a horizontal scrollbar; the chosen context level
  (±3 / ±25 / Full) sticks across files and sessions.
- **Relative times don't run past days** — old commits read `12w` / `8mo` / `2y`
  instead of `730d`.
- **The right-click menu is grouped by subject** (selection · file · commit), and
  hover affordances (🕘, ↗) are faintly visible at rest instead of invisible.

## [0.9.0] — 2026-06-28

### Added

- **Fresh commits show up on their own — auto-fetch on panel open.** When you
  open a single repo, gitwink runs a quiet `git fetch origin` in the background
  so a teammate's just-pushed commits appear without you reaching for the
  terminal. **On by default** (a one-time notice explains it the first time);
  turn it off in Settings → Auto-fetch. It is pinned to `origin` with a
  branch-only refspec and runs with repo hooks disabled, so it only ever
  updates the remote-tracking mirror — never your branches, working tree, or
  history. gitwink still has no merge / push / rewrite features, so it cannot
  damage your work.
- **File history — the commits that touched one file.** Hover a file in the
  diff window and click 🕘 to scope the timeline to that file's history,
  newest first, drawn as a single thread through the revisions. (Renames
  aren't followed across history yet.)

### Changed

- **"Full" diffs of huge files scroll smoothly now.** The side-by-side view is
  virtualized — only the lines on screen are rendered and highlighted — so a
  multi-thousand-line file opens and scrolls without the stutter, and dragging
  the old/new splitter stays fluid.
- **Honest about the network.** gitwink no longer claims "no network" — it was
  never strictly true (the updater checks GitHub), and the new auto-fetch
  reaches your own `origin`. PRIVACY.md and the READMEs now disclose exactly
  what talks to the network — both optional, both to services you already use —
  and lead with what actually matters: fast, lightweight, and read-only where
  it counts.

## [0.8.0] — 2026-06-24

### Added

- **Diff overview minimap + scroll-sync lock** — the side-by-side diff gets a
  change-location rail (add/delete/replace marks; click or drag to jump). The
  two sides scroll together by default; a ⇅ Sync toggle unlocks them to roam
  independently, showing both viewports on the rail with a one-tap re-align.
- **Author filter — focus & sort** — clicking an author now shows only that
  one (matching the branch filter), a hover "only" isolates them from a
  multi-pick, and a Count / A–Z / Recent sort tames the author list when
  viewing all repos / all branches.
- **Update available, in the panel** — an orange badge on the app icon when an
  update is waiting; click it to review and install. Previously only the tray
  surfaced updates. Scoop installs get the badge too (pointing you at
  `scoop update gitwink`).

### Changed

- **The branch list opens instantly in repos with thousands of branches.**
  gitwink no longer walks every branch's history up front to count commits —
  each branch shows its last activity immediately, and commit counts load
  lazily only for the rows you actually scroll past.

### Fixed

- **Long repo / branch names no longer push the header buttons off-screen.**
  The filter chips truncate instead, and in single-repo mode the branch name
  is given priority over the (fixed-context) repo name.
- The commit-search bar now has a clear (×) button.
- **Plain `Ctrl+C`** in an expanded commit copies just your selection, not the
  whole AI-context bundle — that stays on its own dedicated button.

## [0.7.0] — 2026-06-12

### Added

- **Commit search, with a "warp" to context** — press `/` (or `Ctrl+F`), or
  click the new ⌕ button in the header. The timeline itself becomes the
  result list: words match the commit message (full body), author, and repo
  name, all words must match, and a 4+ character hex term matches hashes by
  prefix. The repo scope is respected; the time window and author filters
  are bypassed while searching (shown dimmed) — the commit you're hunting
  must never hide behind "30d". Press Enter (or the row's ↗) to warp: you
  land in that commit's repo history with the branch filter reset, the
  author filter cleared, and the time window widened just enough to cover
  it — the commit arrives centered, selected, with a highlight pulse. Esc
  steps back: first to the results, then to the view you started from.
- Empty states offer "Search commits" — staring at "no commits match" while
  hunting for one is exactly the moment to learn search exists.

### Fixed

- **The single-repo graph no longer shatters under an author filter.**
  Filtering by author used to leak one graph lane per merge — dots
  staircasing rightward over a bundle of full-height vertical lines, on
  merge-heavy repos within one screenful. Hidden commits are now bridged:
  parent links re-route to the nearest visible ancestor and draw dashed
  ("history elided here"), so the graph stays one coherent set of lanes.

Search covers the scanned commit cache; exact lookup of arbitrary old SHAs
(and warping beyond the loaded window) is planned as the next step.

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
