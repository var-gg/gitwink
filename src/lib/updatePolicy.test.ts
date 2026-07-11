import { describe, expect, it } from "vitest";

import type { UpdatePolicy, UpdateStatePayload } from "../types";
import {
  automaticCheckDescription,
  shouldPresentUpdate,
  updateModalTitle,
  updateSettingsSummary,
} from "./updatePolicy";

const standalone: UpdatePolicy = {
  channel: "standalone",
  checkProviderLabel: "verified gitwink releases on GitHub",
  supportsBackgroundChecks: true,
  installAction: { kind: "inApp", providerLabel: "gitwink" },
};

const scoop: UpdatePolicy = {
  channel: "scoop",
  checkProviderLabel: "verified gitwink releases on GitHub",
  supportsBackgroundChecks: true,
  installAction: {
    kind: "externalCommand",
    providerLabel: "Scoop",
    command: "scoop update gitwink",
  },
};

describe("update capability presentation", () => {
  it("derives behaviour from capabilities instead of channel names", () => {
    expect(updateModalTitle(standalone.installAction)).toBe("gitwink update");
    expect(updateModalTitle(scoop.installAction)).toBe("Update available");
    expect(updateSettingsSummary(scoop)).toContain("Scoop owns installation");
    expect(automaticCheckDescription(scoop)).toContain(
      "verified gitwink releases on GitHub",
    );
  });

  it("never opens an empty modal solely because installation is external", () => {
    const empty: UpdateStatePayload = { available: null, policy: scoop };
    expect(shouldPresentUpdate(empty)).toBe(false);

    expect(
      shouldPresentUpdate({
        policy: scoop,
        available: {
          version: "1.2.3",
          notes: "Ready",
          mandatory: false,
        },
      }),
    ).toBe(true);
  });
});
