# Work review validation — 2026-10-31

Implementation base: `76220993db33ca6b92ba7771c05dc90e664aaf60`.
Validated product source: `78c3c29d5ac457ef009f6b2ca2d5eab2457b4962`.
The subsequent documentation commit does not change the executable.
Per-commit Windows/macOS CI results are attached to
[PR #12](https://github.com/var-gg/gitwink/pull/12/checks).

## Automated checks

| Check | Result |
| --- | --- |
| `cargo test --locked --lib` | 207 passed |
| `pnpm test` | 148 passed in 28 owning test files |
| TypeScript and `pnpm build` | Passed |
| `node tools/check-i18n.mjs` | 433 messages complete in English, Korean and Japanese |
| `pnpm tauri build --debug --no-bundle` | Windows executable built with production frontend assets |

The frontend count excludes unrelated nested worktrees. CI now runs frontend
tests and production builds as well as Rust tests/builds on Windows and macOS.
Existing Shiki chunk-size and Rust dead-code warnings remain non-failing.

Regression coverage includes renamed paths and exact revisions, absent versus
oversized/LFS image states, complete-line patch limits, UTF-8 clipboard limits,
net ranges, unborn repositories, staged/unstaged/untracked separation,
unchanged Git state after reads, concurrent edits, moved HEAD, history rewind,
baseline compare-and-swap, and copy batch limits.

An asynchronous parsing/scroll regression test was also run against the
pre-fix component: it failed with the mirrored scroll still at zero. The same
test passes after the viewport observer and scroll listeners attach when
worker-produced rows mount.

## Windows interaction checks

- Cold CLI launch and handoff to an existing review window showed the exact
  resolved commit range. CLI acknowledgement remains explicitly `queued`.
- Current changes displayed unstaged and untracked groups. Untracked files
  started unselected and did not open automatically.
- Editing the working tree after opening a snapshot produced the explicit
  refresh-required error when reading it again.
- Copying a selected range produced 53,272 UTF-8 bytes, contained both full
  revision hashes and 48 omission notes, and excluded untracked files.
- Marking `331c6b9eb66cd00222586b830dd1305fc8a03eae` reviewed on the implementation
  branch saved that baseline and showed zero changes. After `b5da164`, Refresh
  showed exactly its three changed files with the original baseline intact.
- Switching from +/-3 to +/-25 expanded the displayed patch context.
- A temporary 832,000-byte, 16,000-line text file used the worker path. After
  fixing the observer/listener lifecycle, overview navigation and wheel
  scrolling rendered the final line (`15999`, zero-based) correctly. The
  temporary file was removed after verification.
- Review path/hash typography was checked using the configured diff font.

These are Windows interaction results. macOS coverage is automated CI;
interactive macOS acceptance has not been performed here.

## Parser measurement

Node 24.15.0, synthetic all-deletion patches, three runs per size. These measure
`parseDiff`, not repository scanning, worker transfer, highlighting or complete
window latency.

| Deleted lines | Patch bytes | Original parser, ms | Implemented parser, ms |
| ---: | ---: | --- | --- |
| 120,000 | 1,200,021 | 1,223 / 346 / 969 | 26.9 / 11.5 / 12.8 |
| 240,000 | 2,400,021 | 3,675 / 8,080 / 3,567 | 29.1 / 37.9 / 16.5 |
| 480,000 | 4,800,021 | 84,346 / 21,094 / 21,073 | 82.3 / 28.0 / 63.1 |

## Review build identity and boundaries

SHA-256 of the locally verified debug executables:

```text
gitwink.exe     af33556135597736bc2bf0a4c3bba715749e4707202a151f387e7379795ed7e2
gitwinkctl.exe  975cf4b33a93d2c10d2f75fb0b5075e38907331e51965f09dba01ba896b25956
```

Work review displays binary metadata; the existing commit diff window retains
image previews. Current snapshots reject conflicted indexes and enforce
10,000-file/256 MiB content limits. Clipboard output is bounded at 64 KiB;
patch previews at 5 MiB. Review metadata is local and never commits or changes
the index, worktree or refs. Public release/store submission is outside this
implementation. See [Agent control](agent-control.md) for the full contract.
