// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { setupI18n } from "@lingui/core";
import { I18nProvider } from "@lingui/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flattenDiff } from "../lib/diffView";
import { parseDiff } from "../lib/diff";
import { SideBySideDiff } from "./SideBySideDiff";

const delayed = vi.hoisted(() => ({ ready: false, parsed: null as ReturnType<typeof flattenDiff> | null }));
const empty = flattenDiff([]);
vi.mock("../lib/useParsedDiff", () => ({
  useParsedDiff: () => ({
    ...(delayed.ready ? delayed.parsed! : empty),
    parsing: !delayed.ready,
    parseError: null,
  }),
}));

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  delayed.ready = false;
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("requestAnimationFrame", (fn: FrameRequestCallback) => setTimeout(() => fn(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(360);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it("binds virtual scrolling after an asynchronous parse first mounts the columns", async () => {
  const text = "@@ -0,0 +1,2000 @@\n" + Array.from({ length: 2000 }, (_, i) => `+fixture line ${i}\n`).join("");
  delayed.parsed = flattenDiff(parseDiff(text).hunks);
  const i18n = setupI18n({ locale: "en", messages: {} });
  const view = () => <I18nProvider i18n={i18n}>
    <SideBySideDiff text={text} filePath="fixture.txt" fileKey="fixture" locked />
  </I18nProvider>;
  await act(async () => root.render(view()));
  expect(container.querySelector(".sbs-col")).toBeNull();

  delayed.ready = true;
  await act(async () => root.render(view()));
  const [left, right] = Array.from(container.querySelectorAll<HTMLDivElement>(".sbs-col"));
  expect(right).toBeDefined();
  await act(async () => {
    right.scrollTop = 18_000;
    right.dispatchEvent(new Event("scroll"));
    await vi.runOnlyPendingTimersAsync();
  });
  const visible = Array.from(right.querySelectorAll<HTMLElement>("[data-line-num]"))
    .map(row => Number(row.dataset.lineNum)).filter(Boolean);
  expect(left.scrollTop).toBe(18_000);
  expect(visible.length).toBeGreaterThan(15);
  expect(visible.length).toBeLessThan(60);
  expect(Math.min(...visible)).toBeGreaterThan(980);
  expect(Math.max(...visible)).toBeLessThan(1040);
});
