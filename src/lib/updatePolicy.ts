import type {
  UpdateInstallAction,
  UpdatePolicy,
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
    return `Checks use ${policy.checkProviderLabel}. ${install.providerLabel} owns installation.`;
  }
  return `Checks use ${policy.checkProviderLabel}. Updates install in-app through ${install.providerLabel}.`;
}

export function automaticCheckDescription(policy: UpdatePolicy): string {
  return `Check ${policy.checkProviderLabel} on startup + every 24h. Tray dot when one’s ready.`;
}

export function updateModalTitle(action: UpdateInstallAction): string {
  return action.kind === "inApp"
    ? `${action.providerLabel} update`
    : "Update available";
}

export function waitingForProviderLabel(action: UpdateInstallAction): string {
  return action.providerLabel;
}
