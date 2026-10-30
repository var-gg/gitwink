# Agent control surface

gitwink exposes a small, read-only command surface so coding agents can hand
precise Git evidence to the user without requiring MCP, an editor extension,
or a vendor-specific skill. The command is an optional presentation aid; an
agent's answer must still make sense when gitwink is not installed.

## Discover the installed capability

Prefer the console companion when it is available:

```sh
gitwinkctl capabilities --json
gitwinkctl help agents
```

`capabilities --json` is the machine-readable source of truth. Do not infer a
newer action from the app version. Commands listed under
`plannedNotAvailable` are deliberately not implemented yet.

Packaged direct-download builds place `gitwinkctl` beside the gitwink app.
Package managers may expose it on `PATH`. A Microsoft Store package exposes
the GUI activation alias `gitwink`; the GUI binary accepts the same `show ...`
arguments but has no console output. On macOS the packaged companion lives at
`/Applications/gitwink.app/Contents/MacOS/gitwinkctl`; users who want a global
command can symlink it into a directory already on `PATH`.

## Show a commit

```sh
gitwinkctl show commit --repo /path/to/repo --rev <commit-or-ref> --source Codex
```

The Windows form is identical:

```powershell
gitwinkctl show commit --repo A:\projects\gitwink --rev HEAD --source Codex
```

The commit is resolved directly from the local repository, independently of
the normal timeline's newest-N and time-window limits. gitwink switches to the
repository, clears branch and author filters that could hide the target,
widens the time range, centres and pulses the exact row, and expands it by
default. Use `--no-expand` for a collapsed landing.

## Show one file's history

```sh
gitwinkctl show file-history --repo /path/to/repo \
  --path src/lib/controlHandoff.ts --source Claude-Code
```

`--path` is always repository-relative. Absolute paths, `..`, empty path
components, and paths that escape the working tree are rejected. A path need
not exist in the current checkout; history for a deleted file is still a valid
question. Rename following is not available in the first protocol version and
is reported as such by `capabilities` and the UI.

## Show a work review

```sh
gitwinkctl show range --repo /path/to/repo --base <base-commit> --head <head-commit> --source Codex
gitwinkctl show worktree --repo /path/to/repo --source Codex
```

Ranges compare the exact two resolved commits (net tree change, not concatenated
commit patches or an implicit merge base). Current changes show staged,
unstaged and untracked groups separately. An uncommitted snapshot is checked
against HEAD, the index and changed file contents before reading/copying;
concurrent edits require Refresh. Both commands open the same Work review
window available from a selected repository in the panel.

The window can save a local reviewed baseline per repository worktree and
branch. It advances only when explicitly requested for the inspected current
HEAD. Worktree snapshots never advance a committed baseline. A missing baseline
starts with no implied reviewed history; changed/replaced history requires an
explicit range. Review metadata lives in `review.sqlite` in the app data folder.

Copying includes full base/head hashes, file groups, selected paths and omission
notes within 64 KiB. Untracked files start unselected. Binary contents are
metadata-only. Text patches are capped at 5 MiB, copied patches at 32 files and
1,200 changed lines per file. A current-changes snapshot supports up to 10,000
changed files and hashes up to 256 MiB of local changed file content; larger
snapshots and conflicted indexes report a clear limit instead of a partial view.

## GUI activation fallback

If `gitwinkctl` is not exposed on `PATH` but the app binary or Store alias is,
the visual handoff still works:

```sh
gitwink show commit --repo /path/to/repo --rev <commit> --source Codex
gitwink show file-history --repo /path/to/repo --path src/App.tsx --source Codex
```

This form intentionally prints no result. `gitwinkctl` is preferred for
automation because it returns a stable exit code and optional JSON envelope.

## Delivery and safety contract

- `--repo` defaults to the caller's current working directory and may point to
  any subdirectory of a working tree.
- The repository must already be registered, active, and readable in gitwink.
  A command never auto-adds, restores, clones, or changes a repository to make
  a request succeed.
- Show actions are read-only. They never commit, checkout, merge, reset, push,
  or rewrite files. Existing app update checks and the user's configured
  auto-fetch policy remain separate from the control request.
- Several requests arriving together are latest-wins. Reusing the same
  `--request-id` is idempotent within the running app.
- A successful CLI result means `queued`, not “rendered”. Resolution errors
  such as an unknown revision or excluded repository are surfaced visibly in
  the panel. Structured end-to-end acknowledgement is not claimed in v1.
- Exit code `0` means the activation was queued, `2` means invalid arguments,
  and `3` means the app could not be located or launched.

For scripts, add `--json` to a `show` command:

```json
{
  "protocol": "gitwink.control.v1",
  "status": "queued",
  "requestId": "local-...",
  "target": "commit HEAD",
  "acknowledged": false
}
```

## Guidance for coding agents

Use the control surface when the visual handoff materially helps: showing the
commit just produced, opening the history of a file the user asked to trace,
or letting the user inspect evidence without reading a long prose dump. Do not
open gitwink for every answer, do not make it a prerequisite for correctness,
and fall back to a concise textual summary when capability discovery fails.

Portable repository identities, shareable view links, inspection queries and
render acknowledgements remain unavailable.
