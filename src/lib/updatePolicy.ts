import type {
  UpdateInstallAction,
  UpdatePolicy,
  UpdateProvider,
  UpdateStatePayload,
} from "../types";

/** True only when a concrete available update exists. Package ownership alone
 * must never summon an empty modal. */
export function shouldPresentUpdate(state: UpdateStatePayload): boolean {
  return state.available != null;
}

export function updateSettingsSummary(policy: UpdatePolicy): string {
  const install = policy.installAction;
  if (install.kind === "externalCommand") {
    return `Checks use ${updateProviderDefaultLabel(policy.checkProvider)}. ${updateProviderDefaultLabel(install.provider)} owns installation.`;
  }
  return `Checks use ${updateProviderDefaultLabel(policy.checkProvider)}. Updates install in-app through ${updateProviderDefaultLabel(install.provider)}.`;
}

export function automaticCheckDescription(policy: UpdatePolicy): string {
  return `Check ${updateProviderDefaultLabel(policy.checkProvider)} on startup + every 24h. Tray dot when one’s ready.`;
}

export function updateModalTitle(action: UpdateInstallAction): string {
  return action.kind === "inApp"
    ? `${updateProviderDefaultLabel(action.provider)} update`
    : "Update available";
}

export function waitingForProviderLabel(action: UpdateInstallAction): string {
  return updateProviderDefaultLabel(action.provider);
}

export function updateProviderDefaultLabel(provider: UpdateProvider): string {
  switch (provider) {
    case "githubReleases":
      return "verified gitwink releases on GitHub";
    case "microsoftStore":
      return "Microsoft Store";
    case "scoop":
      return "Scoop";
    case "gitwink":
      return "gitwink";
  }
}
