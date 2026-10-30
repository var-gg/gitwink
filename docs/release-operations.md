# Release verification and recovery

Record each release's source commit, workflow runs, artifact hashes and channel
results in `docs/releases/<version>.json`. Preserve the tagged source commit
when subsequent documentation records the result.

## GitHub and the in-app updater

`release.yml` builds Windows MSI/NSIS and universal macOS packages into a draft.
The workflow verifies the combined `latest.json` before publishing. After
publication, download the public assets, verify their hashes and updater
signatures against the checked-in public key, and check the public latest
endpoint. The MSI and NSIS routes must retain their installer families. Verify
the packaged `gitwinkctl` version and capabilities; both macOS binaries must
contain Intel and Apple Silicon architectures.

Do not publish an incomplete draft or move a public version tag to repair a
build. Rerun failed jobs only when their previous external effects are known.

## Scoop

The live manifest is `bucket/gitwink.json` in `var-gg/scoop-bucket` on `master`.
After the GitHub release is public, dispatch its `excavator.yml` workflow to
update immediately. Verify the public manifest version, installer URL and
SHA-256 against the published MSI. The `packaging/scoop` file in this repository
is an initial template, not the live bucket.

## WinGet

The release workflow opens a PR in `microsoft/winget-pkgs` for `var-gg.gitwink`.
Check the version, NSIS URL and SHA-256 in that exact PR. Submission success is
not upstream publication: verify the PR is merged and its version manifest is
present upstream. Client catalog propagation can follow the merge.

If fork synchronization fails because an older PAT lacks `workflow` scope,
use an existing authorized GitHub connection to fast-forward
`var-gg/winget-pkgs`'s `master` to upstream with an expected-head lease and force
disabled. Then rerun only the failed winget job. Check for an existing version
PR before any manual resubmission. No new token scope is required when an
existing authorized connection can perform the fast-forward.

## Microsoft Store

Use the exact MSIX artifact from the tagged Store workflow. Verify its product
identity, version and x64/ARM64 contents. Follow the exact submission ID through
the [read-only status probe](store-listing-automation.md#read-only-release-follow-up).
A successful workflow means ingestion was accepted; certification and public
publication are separate asynchronous results. Never re-upload or recommit an
in-flight submission merely to check its progress.
