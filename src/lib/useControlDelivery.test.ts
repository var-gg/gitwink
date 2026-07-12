import { describe, expect, it, vi } from "vitest";

import type { ControlDelivery } from "./ipc";
import { createControlDeliveryBroker } from "./useControlDelivery";

function delivery(requestId: string): ControlDelivery {
  return {
    status: "error",
    requestId,
    source: { kind: "cli", name: "test" },
    error: { code: "test", message: "test" },
  };
}

describe("control delivery broker", () => {
  it("hands a cold delivery to the replacement StrictMode subscriber", async () => {
    let releaseDrain!: (deliveries: ControlDelivery[]) => void;
    const drainNative = vi.fn(
      () =>
        new Promise<ControlDelivery[]>((resolve) => {
          releaseDrain = resolve;
        }),
    );
    const broker = createControlDeliveryBroker(
      drainNative,
      async () => () => undefined,
    );
    const abandoned: string[] = [];
    const received: string[] = [];

    const unsubscribe = broker.subscribe((item) =>
      abandoned.push(item.requestId),
    );
    await vi.waitFor(() => expect(drainNative).toHaveBeenCalledOnce());
    unsubscribe();
    broker.subscribe((item) => received.push(item.requestId));
    releaseDrain([delivery("cold")]);

    await vi.waitFor(() => expect(received).toEqual(["cold"]));
    expect(abandoned).toEqual([]);
    broker.dispose();
  });

  it("publishes only the latest delivery from one native drain", async () => {
    const broker = createControlDeliveryBroker(
      async () => [delivery("older"), delivery("latest")],
      async () => () => undefined,
    );
    const received: string[] = [];
    broker.subscribe((item) => received.push(item.requestId));

    await vi.waitFor(() => expect(received).toEqual(["latest"]));
    broker.dispose();
  });

  it("deduplicates a repeated request across wake drains", async () => {
    const repeated = delivery("same");
    const pulls = [[repeated], [repeated]];
    let wake!: () => void;
    const drainNative = vi.fn(async () => pulls.shift() ?? []);
    const broker = createControlDeliveryBroker(drainNative, async (callback) => {
      wake = callback;
      return () => undefined;
    });
    const received: string[] = [];
    broker.subscribe((item) => received.push(item.requestId));
    await vi.waitFor(() => expect(received).toEqual(["same"]));

    wake();
    await vi.waitFor(() => expect(drainNative).toHaveBeenCalledTimes(2));
    expect(received).toEqual(["same"]);
    broker.dispose();
  });
});
