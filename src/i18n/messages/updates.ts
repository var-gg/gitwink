import { msg } from "@lingui/core/macro";
import type { I18n } from "@lingui/core";

import type { UpdateProvider } from "../../types";

export const updateMessages = {
  available: msg({ id: "update.available", message: "Update available" }),
  managedExternally: msg({
    id: "update.managedExternally",
    message:
      "This installation is managed by {provider}, which owns the final update step. Run {command} in a terminal to get v{version}.",
  }),
  gotIt: msg({ id: "common.gotIt", message: "Got it" }),
  failed: msg({ id: "update.failed", message: "Update failed" }),
  inAppTitle: msg({ id: "update.inAppTitle", message: "{provider} update" }),
  noNotes: msg({
    id: "update.noNotes",
    message: "No release notes were provided for this version.",
  }),
  progress: msg({ id: "update.progress", message: "Update progress" }),
  updateNow: msg({ id: "update.now", message: "Update now" }),
  later: msg({ id: "update.later", message: "Later" }),
  skip: msg({ id: "update.skip", message: "Skip v{version}" }),
  waiting: msg({ id: "update.waiting", message: "Waiting for {provider}…" }),
  downloading: msg({ id: "update.downloading", message: "Downloading…" }),
  installing: msg({ id: "update.installing", message: "Installing…" }),
  restarting: msg({ id: "update.restarting", message: "Restarting…" }),
  preparing: msg({ id: "update.preparing", message: "Preparing…" }),
  dialog: msg({ id: "update.dialog", message: "gitwink update" }),
  providerGithub: msg({
    id: "update.provider.githubReleases",
    message: "verified gitwink releases on GitHub",
  }),
  providerMicrosoftStore: msg({
    id: "update.provider.microsoftStore",
    message: "Microsoft Store",
  }),
  providerScoop: msg({ id: "update.provider.scoop", message: "Scoop" }),
  providerGitwink: msg({ id: "update.provider.gitwink", message: "gitwink" }),
} as const;

export function updateProviderLabel(
  i18n: I18n,
  provider: UpdateProvider,
): string {
  switch (provider) {
    case "githubReleases":
      return i18n._(updateMessages.providerGithub);
    case "microsoftStore":
      return i18n._(updateMessages.providerMicrosoftStore);
    case "scoop":
      return i18n._(updateMessages.providerScoop);
    case "gitwink":
      return i18n._(updateMessages.providerGitwink);
  }
}
