# gitwink architecture

> Snapshot for v0.1. Lives next to the code; update when it drifts.

## Layers

```
+-----------------------------------------------------+
|  React + TypeScript (panel UI)                      |
|  src/                                               |
|    components/  Timeline · CommitDetail · DiffView  |
|    lib/ipc.ts   typed wrappers over @tauri-apps/api |
|    types.ts     mirrored from Rust serde structs    |
+--------------------------|--------------------------+
                           | invoke() / events
+--------------------------v--------------------------+
|  Rust (Tauri 2 backend)                             |
|  src-tauri/src/                                     |
|    commands.rs   Tauri command handlers             |
|    discovery.rs  ignore + walkdir repo scan         |
|    git.rs        git2 read-only wrappers            |
|    cache.rs      rusqlite layer                     |
|    tray.rs       tray icon + menu                   |
|    window.rs     panel show/hide/position           |
+-----------------------------------------------------+
```

## Hard rules

- **Read-only against your work.** gitwink never commits, merges, rebases,
  pushes, or rewrites — it cannot alter or lose your repo. `git.rs` must never
  call any `git2` API that mutates a repo or touches the network (no commits,
  no fetch, no merges, no working-tree writes); libgit2 here is built without
  a network transport. The one network touch against a repo is the panel-open
  fetch (on by default, toggleable) — it shells out to the system `git` in
  `fetch.rs`, pinned to `origin` with a branch-only refspec so it can only
  write `refs/remotes/origin/*`, with repo hooks disabled and submodule
  recursion off, and still never touches branches, the working tree, or
  history.
- **No telemetry, no analytics, no auto-uploads**, and no repo data ever
  leaves the machine to us. The only outbound network is the self-updater
  (GitHub Releases) and the panel-open auto-fetch (your repo's own `origin`,
  via your own Git credentials).
- **Cold start ≤300ms / idle RAM ≤100MB.** Architecture preserves this:
  - Lazy-load git data, eager-load only the cached timeline.
  - No long-lived per-repo polling; rely on `notify` (v0.2) for refresh.
- **Errors are silent unless critical.** Repo failing to load → small
  indicator next to that repo's commits. Never blocks the UI.

## Data locations

- SQLite cache: `%APPDATA%\gitwink\cache.db` (Windows) /
  `~/Library/Application Support/gitwink/cache.db` (macOS)
- Settings JSON: same directory, `settings.json`

## v0.1 → v0.2 sketch

v0.1 ships the homepage timeline, commit drill-down, diff view, and "Copy as
AI context." v0.2 adds the `notify`-backed file watcher (so the badge updates
without re-scan), the global hotkey binding, and a custom commit graph.
