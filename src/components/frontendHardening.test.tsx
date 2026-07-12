import { renderToStaticMarkup } from "react-dom/server";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { setupI18n } from "@lingui/core";
import { I18nProvider } from "@lingui/react";

import {
  BRANCH_COUNT_MAX_RETRIES,
  branchCountRetryDelay,
} from "./BranchChip";
import { contextMenuDismissesOnTab } from "./ContextMenu";
import { SearchBar } from "./SearchBar";
import { SearchResultJumpAffordance } from "./TimelineWindowed";
import {
  canDeferUpdate,
  modalTabDestination,
  updateProgressLabel,
} from "./UpdateModal";
import { updateCheckNoticeLabel } from "./header/PanelChromeRow";
import {
  SEARCH_RESULTS_LIST_ID,
  searchResultOptionId,
} from "./searchAccessibility";

const noop = () => {};
const testI18n = setupI18n({ locale: "en", messages: {} });

function renderLocalized(node: ReactNode) {
  return renderToStaticMarkup(
    <I18nProvider i18n={testI18n}>{node}</I18nProvider>,
  );
}

function renderSearch(overrides?: {
  resultsAvailable?: boolean;
  resultsBusy?: boolean;
  activeDescendant?: string | null;
}) {
  return renderLocalized(
    <SearchBar
      value="fix race"
      count={2}
      focusNonce={0}
      scopeLabel="All repos · all history"
      resultsListId={SEARCH_RESULTS_LIST_ID}
      resultsAvailable={overrides?.resultsAvailable ?? true}
      resultsBusy={overrides?.resultsBusy ?? false}
      activeDescendant={
        overrides?.activeDescendant ?? searchResultOptionId(0)
      }
      onChange={noop}
      onClose={noop}
      onMove={noop}
      onActivate={noop}
    />,
  );
}

describe("branch count retry budget", () => {
  it("backs off exponentially and clamps at the bounded retry budget", () => {
    expect(branchCountRetryDelay(1)).toBe(300);
    expect(branchCountRetryDelay(2)).toBe(600);
    expect(branchCountRetryDelay(3)).toBe(1_200);
    expect(branchCountRetryDelay(BRANCH_COUNT_MAX_RETRIES + 10)).toBe(1_200);
  });
});

describe("overlay keyboard ownership", () => {
  it("dismisses a context menu before Tab can escape underneath it", () => {
    expect(contextMenuDismissesOnTab("Tab")).toBe(true);
    expect(contextMenuDismissesOnTab("ArrowDown")).toBe(false);
  });

  it("keeps the pointer jump glyph presentational inside a listbox option", () => {
    const markup = renderLocalized(
      <SearchResultJumpAffordance onActivate={noop} />,
    );
    expect(markup).toContain('class="row-jump"');
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).not.toContain("<button");
  });

  it("keeps focus in a busy modal and wraps at both live edges", () => {
    expect(modalTabDestination(0, -1, false)).toBe("dialog");
    expect(modalTabDestination(3, 2, false)).toBe(0);
    expect(modalTabDestination(3, 0, true)).toBe(2);
    expect(modalTabDestination(3, 1, false)).toBeNull();
  });

  it("describes update phases without exposing backend state names", () => {
    expect(updateProgressLabel(null)).toBe("Preparing…");
    expect(
      updateProgressLabel(
        { phase: "waiting", progress: 0 },
        "Microsoft Store",
      ),
    ).toBe("Waiting for Microsoft Store…");
    expect(updateProgressLabel({ phase: "downloading", progress: 0.42 })).toBe(
      "Downloading…",
    );
    expect(updateProgressLabel({ phase: "installing", progress: 0.9 })).toBe(
      "Installing…",
    );
    expect(updateProgressLabel({ phase: "restarting", progress: 1 })).toBe(
      "Restarting…",
    );
  });

  it("keeps every manual update-check outcome visible and concise", () => {
    expect(updateCheckNoticeLabel({ kind: "checking" })).toBe(
      "Checking updates…",
    );
    expect(updateCheckNoticeLabel({ kind: "current" })).toBe("✓ Up to date");
    expect(updateCheckNoticeLabel({ kind: "error" })).toBe(
      "⚠ Update check failed",
    );
  });

  it("never offers skip or snooze for a mandatory Store update", () => {
    expect(canDeferUpdate({ mandatory: true })).toBe(false);
    expect(canDeferUpdate({ mandatory: false })).toBe(true);
  });
});

describe("commit-search accessibility contract", () => {
  it("connects the combobox to the mounted selected option", () => {
    const markup = renderSearch();
    expect(markup).toContain('role="combobox"');
    expect(markup.toLowerCase()).toContain('maxlength="512"');
    expect(markup).toContain(`aria-controls="${SEARCH_RESULTS_LIST_ID}"`);
    expect(markup).toContain('aria-expanded="true"');
    expect(markup).toContain('aria-busy="false"');
    expect(markup).toContain('aria-describedby="commit-search-scope"');
    expect(markup).toContain('id="commit-search-scope"');
    expect(markup).toContain(
      `aria-activedescendant="${searchResultOptionId(0)}"`,
    );
  });

  it("withholds stale option ownership while a replacement query resolves", () => {
    const markup = renderSearch({ resultsBusy: true });
    expect(markup).toContain('aria-busy="true"');
    expect(markup).not.toContain("aria-activedescendant");
  });

  it("does not point at a listbox before the debounced query mounts it", () => {
    const markup = renderSearch({ resultsAvailable: false });
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).not.toContain("aria-controls");
    expect(markup).not.toContain("aria-activedescendant");
  });
});
