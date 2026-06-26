# gitwink Privacy Policy

_Last updated: 2026-06-26_

gitwink is a tray-resident, read-only tool for glancing at recent commit
activity across your local Git repositories.

## Summary

gitwink is **read-only** — it never merges, pushes, rebases, or rewrites, so
it cannot alter or lose your work. It has no telemetry or analytics and sends
no information about you or your repositories to us. Its network use is
limited and goes only to services you already use: a check for app updates
(GitHub) and an automatic `git fetch` of the repository you're viewing — on
by default, talking to that repo's own `origin` remote via your own Git
credentials, and easily turned off. See [Network activity](#network-activity)
below.

## What gitwink accesses

To do its job, gitwink reads — locally, on your machine — the Git
repositories you point it at or that it discovers in common project
folders:

- Commit metadata (messages, author names and email addresses,
  timestamps, branch and tag names) from each repository's Git history.
- File contents and diffs, when you open a commit to inspect it.

This information is read directly from your local `.git` directories and is
**never** sent to us or any third party. (When auto-fetch is on — the default
— a `git fetch` talks only to that repository's `origin` remote; see
[Network activity](#network-activity).)

## What gitwink stores

gitwink keeps a local cache and your settings on your own machine, under
your user profile (`%APPDATA%\gg.var.gitwink` on Windows):

- `cache.db` — a SQLite cache of repository and commit data so the app
  paints quickly.
- `settings.json` — your preferences (panel position, pinned
  repositories, update-check mode, the auto-fetch toggle, and similar).

These files never leave your computer. Uninstalling gitwink or deleting
that folder removes them.

## Network activity

gitwink has no account, no telemetry, no analytics, and no advertising, and
it sends no information about you or your repositories to us. The only network
activity it initiates is two features — both to services you already use, and
both easily turned off:

1. **App update check** — gitwink may contact GitHub to see whether a newer
   release is available and, if you choose to update, to download it. These
   requests go to GitHub and are subject to
   [GitHub's Privacy Statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement);
   no information about you or your repositories is included in them. Set the
   update checker to manual or off in `settings.json`.
2. **Auto-fetch on panel open** _(on by default)_ — when you're viewing a
   single repository, gitwink runs a quiet `git fetch` as the panel opens, so a
   teammate's just-pushed commit shows up. It uses your system `git` and your
   existing Git credentials, pinned to that repository's `origin` remote with a
   branch-only refspec — gitwink adds no destination of its own and sends
   nothing beyond what a normal fetch negotiates (e.g. which commit IDs you
   already have). Locally it updates only the remote-tracking mirror
   (`refs/remotes/origin/*`); it never touches your branches, working tree, or
   history, and never pushes, merges, or rewrites. A repository without an
   `origin` remote isn't fetched. Turn it off in Settings → Auto-fetch (or
   `settings.json`).

## Third-party components

- gitwink's interface is rendered with Microsoft Edge WebView2, a Windows
  component governed by Microsoft's privacy terms.
- Update checks and downloads are served by GitHub, as described above.

## Children's privacy

gitwink is a developer tool, is not directed at children, and collects no
personal information from anyone.

## Changes to this policy

If this policy changes, the updated version will be published at this
same URL with a new "Last updated" date.

## Contact

Questions about privacy in gitwink: **admin@var.gg**

Issues and source code: <https://github.com/var-gg/gitwink>
