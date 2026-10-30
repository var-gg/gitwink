# Work review implementation

Approved scope: repair the diff correctness/performance findings from the
2026-10-31 review, then add review of a commit range and current uncommitted
changes, a local reviewed baseline, bounded AI context, and CLI handoff.

## Completion criteria

- Renamed text compares the correct old/new paths; literal filenames remain safe.
- Diff limits and image availability are explicit in both display and copying.
- Large diff parsing is linear and runs off the UI thread for large inputs.
- Existing frontend suites run; CI includes frontend tests and build.
- A selected repository can review exact base/head commits, its saved reviewed
  baseline, and staged/unstaged/untracked changes without modifying Git state.
- A baseline is only advanced by an explicit user action for the inspected
  snapshot. Rewrites, repository replacement and concurrent edits are detected.
- Copying states exact revisions, selected files and omissions under a total
  byte budget. CLI range/worktree handoffs use the same review surface.
- English, Korean and Japanese wording; relevant tests, production build and
  Windows interaction checks. Public release submission is a separate action.

## Module boundaries

Extract patch/blob commands and data access from the existing large Git and
command modules. Keep review domain, persistence, IPC, window and React UI in
separate modules. Add only routing integration to the existing panel controller.
Keep CSS local to the review surface. Preserve current read-only/privacy policy.

## Work units

1. Diff result/preview contract, rename fix, bounded copying, parser/worker,
   regression tests and the frontend CI gate.
2. Review snapshots, exact range/worktree reads and local baseline persistence.
3. Review UI, localization, bounded context and CLI handoff.
4. Integration verification and documentation.

## Progress

- 2026-10-31: baseline HEAD `7622099`; tracked worktree clean. Existing `.agents/`
  is untracked and outside this change. Baseline review receipts are retained in
  the local review report. Implementation begins on `codex/work-review`.

- Diff foundation complete: exact rename provenance, explicit patch/image limits,
  bounded copy, linear worker parsing, and CI frontend gates. Verified 200 Rust
  tests, 171 frontend tests, three complete locales and production build.

- Native review foundation complete: exact net ranges, branch/worktree-scoped
  baseline CAS, and read-only staged/unstaged/untracked snapshots. Five new
  regression tests pass, including concurrent edits, history rewind, unborn
  repositories, and unchanged index/HEAD/worktree after review reads.

- Review window/CLI complete: three modes, explicit baseline action, grouped
  files, bounded selection copy, native pending handoff and all three locales.
  Validation: 207 Rust tests, 175 frontend tests, 433 translated messages and
  Windows debug production-asset build pass. Native range/worktree rendering
  and stale-snapshot errors observed. Final interaction checks continue.

- First macOS CI exposed a pre-existing Windows-only case assumption in a
  handoff test. Corrected the test fixture and scoped Vitest to src/tools;
  the earlier local 175 total included 28 tests in an unrelated nested
  worktree. The owning frontend suite is now 147 tests, all passing locally.
