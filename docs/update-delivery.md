# Update delivery architecture

gitwink is distributed through several storefronts and package managers, but
the place a user typed an install command is not always observable—or relevant—
after installation. The updater therefore models three separate decisions:

1. **Install channel** describes who owns the installed artifact.
2. **Check backend** describes where update availability comes from.
3. **Install action** describes whether gitwink may install in-app or must hand
   the final step back to an external package manager.

This separation is the update subsystem's compatibility contract. UI code must
consume the serialized capabilities and must not infer behaviour from a channel
name.

## Supported distribution matrix

| Distribution path | Runtime channel | Availability source | Install action |
| --- | --- | --- | --- |
| Microsoft Store MSIX | `store` | Device Microsoft Store catalog | In-app through StoreContext |
| Scoop bucket | `scoop` | Verified GitHub Release metadata | External: `scoop update gitwink` |
| WinGet (`var-gg.gitwink`) | `standalone` | Verified GitHub Release metadata | In-app through Tauri updater |
| Direct Windows NSIS/MSI | `standalone` | Verified GitHub Release metadata | In-app through Tauri updater |
| Direct macOS DMG | `standalone` | Verified GitHub Release metadata | In-app through Tauri updater |

WinGet is intentionally not guessed at runtime. It installs the same NSIS
artifact as a direct download and leaves no durable provenance that changes the
artifact's update contract. Treating both as `standalone` is explicit policy,
not a fallback accident.

## Runtime boundary

The backend resolves one immutable policy during Tauri setup:

- Windows package identity selects `store`. A `WindowsApps` path is retained as
  a defensive fallback.
- A path below `$SCOOP/apps`, `$SCOOP_GLOBAL/apps`, or the conventional
  `scoop/apps` layout selects `scoop`.
- Everything else selects `standalone`, whose documented scope is the WinGet,
  direct-download and macOS artifacts above.

`src-tauri/src/update/policy.rs` is the single policy table.
`src-tauri/src/update/provider.rs` performs provider I/O. `update.rs` owns only
scheduling, publication, suppression, progress and the install lease.

Tauri patches the originating bundle type into each packaged binary. The
release manifest therefore carries separate `windows-x86_64-msi` and
`windows-x86_64-nsis` entries; MSI stays MSI while WinGet/NSIS stays NSIS.
`tools/verify-update-manifest.mjs` guards those routes before a draft GitHub
Release is allowed to become public.

The frontend receives an `UpdatePolicy` capability payload:

- `checkProviderLabel`
- `supportsBackgroundChecks`
- tagged `installAction`: `inApp` or `externalCommand`

The diagnostic `channel` is deliberately insufficient to drive behaviour.
This keeps Store/Scoop-specific knowledge out of React components and prevents
direct IPC calls from bypassing external package ownership.

## Safety invariants

- A Store package never checks GitHub for an update and never invokes the Tauri
  installer.
- An externally managed package can check and badge in-app, but its install IPC
  always returns the package manager's command.
- Mandatory Store updates cannot be skipped or snoozed.
- Disabling checks invalidates in-flight publication and removes every update
  affordance.
- A concrete available version is required before any update modal opens.
- Provider-specific frontend plugin permissions are not exposed; only gitwink's
  policy-enforcing IPC commands can initiate installation.

## Adding a distribution channel

1. Decide whether it creates a genuinely different installed artifact or is
   only another way to obtain an existing one.
2. Add a policy only when its check or install ownership differs.
3. Implement provider I/O without adding scheduling or UI state to the provider.
4. Express frontend behaviour through capabilities; do not add channel-name
   branches to components.
5. Extend the Rust policy matrix and frontend capability tests.
6. Extend the release-manifest validator if a new installer family is added.
7. Validate the real N→N+1 update path on the distribution service; compilation
   and package creation alone do not prove catalog rollout behaviour.

## Microsoft Store release metadata

Package delivery and public listing assets share one Store submission but have
different source-of-truth rules. The package is built by CI; approved
screenshots are checked in and changed only for explicitly listed locales. See
[Microsoft Store listing asset automation](store-listing-automation.md) for the
manifest, validation, draft ownership, and commit guarantees.
