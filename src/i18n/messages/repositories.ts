import { msg } from "@lingui/core/macro";

export const repositoryMessages = {
  title: msg({ id: "settings.repositories.title", message: "Repositories" }),
  intro: msg({
    id: "settings.repositories.intro",
    message:
      "Excluded repositories are ignored by discovery and removed from the panel, but their folders, pin state, and branch preferences remain intact.",
  }),
  loading: msg({ id: "settings.repositories.loading", message: "Loading…" }),
  empty: msg({
    id: "settings.repositories.empty",
    message: "No excluded repositories.",
  }),
  folderCheckedOnRestore: msg({
    id: "settings.repositories.folderCheckedOnRestore",
    message: "Folder availability is checked on restore",
  }),
  folderAvailable: msg({
    id: "settings.repositories.folderAvailable",
    message: "Folder available",
  }),
  folderUnavailable: msg({
    id: "settings.repositories.folderUnavailable",
    message: "Folder unavailable—restores as missing until it returns",
  }),
  excludedAt: msg({
    id: "settings.repositories.excludedAt",
    message: "Excluded {when}",
  }),
  replacementWarning: msg({
    id: "settings.repositories.replacementWarning",
    message:
      "Reviewed: {repo}. Confirmation remains valid only while this exact repository and exclusion lifecycle are unchanged. This forgets gitwink's old cache, pin, and branch choices for this path, then registers the Git repository currently in the folder. The folder itself is never changed.",
  }),
  restore: msg({ id: "settings.repositories.restore", message: "Restore" }),
  restoring: msg({ id: "settings.repositories.restoring", message: "Restoring…" }),
  inspect: msg({
    id: "settings.repositories.inspect",
    message: "Use current repo…",
  }),
  inspecting: msg({ id: "settings.repositories.inspecting", message: "Inspecting…" }),
  confirmReplace: msg({
    id: "settings.repositories.confirmReplace",
    message: "Confirm replace",
  }),
  replacing: msg({ id: "settings.repositories.replacing", message: "Replacing…" }),
  cancel: msg({ id: "common.cancel", message: "Cancel" }),
  restoreAria: msg({
    id: "settings.repositories.restoreAria",
    message: "Restore {repo}",
  }),
  confirmReplaceAria: msg({
    id: "settings.repositories.confirmReplaceAria",
    message: "Confirm replacing {oldRepo} with {newRepo}",
  }),
  cancelReplaceAria: msg({
    id: "settings.repositories.cancelReplaceAria",
    message: "Cancel replacing {repo}",
  }),
  useCurrentAria: msg({
    id: "settings.repositories.useCurrentAria",
    message: "Use the current repository in {repo}'s folder",
  }),
  loadError: msg({
    id: "settings.repositories.loadError",
    message: "Couldn't load excluded repositories",
  }),
  restoreError: msg({
    id: "settings.repositories.restoreError",
    message: "Couldn't restore {repo}",
  }),
  inspectError: msg({
    id: "settings.repositories.inspectError",
    message: "Couldn't inspect {repo}",
  }),
  replaceError: msg({
    id: "settings.repositories.replaceError",
    message: "Couldn't use the current {repo} repository",
  }),
  identityMismatch: msg({
    id: "settings.repositories.identityMismatch",
    message:
      "This folder now contains a different Git repository. Review the replacement details before confirming.",
  }),
  confirmationExpired: msg({
    id: "settings.repositories.confirmationExpired",
    message:
      "The repository changed after you reviewed it. Review the current repository again before confirming.",
  }),
  restoredAnnouncement: msg({
    id: "settings.repositories.restoredAnnouncement",
    message: "{repo} restored.",
  }),
  replacedAnnouncement: msg({
    id: "settings.repositories.replacedAnnouncement",
    message: "{repo} was replaced with the repository currently in its folder.",
  }),
} as const;
