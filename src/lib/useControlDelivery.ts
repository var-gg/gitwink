import { useEffect, useRef } from "react";

import {
  onControlPending,
  takePendingControlDeliveries,
  type ControlDelivery,
} from "./ipc";

type DeliverySubscriber = (delivery: ControlDelivery) => void;
type NativeDrain = () => Promise<ControlDelivery[]>;
type NativeListen = (wake: () => void) => Promise<() => void>;

export interface ControlDeliveryBroker {
  subscribe(subscriber: DeliverySubscriber): () => void;
  dispose(): void;
}

/** Process-lifetime bridge around the native queue. React development mode
 * deliberately mounts, cleans up, and mounts effects again; tying a
 * destructive native drain to either temporary effect can consume a cold
 * activation into an already-disposed component. The broker owns one drain
 * and briefly buffers its latest result across that handover. */
export function createControlDeliveryBroker(
  drainNative: NativeDrain,
  listenNative: NativeListen,
): ControlDeliveryBroker {
  const subscribers = new Set<DeliverySubscriber>();
  let started = false;
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let serializedDrain = Promise.resolve();
  let buffered: ControlDelivery | null = null;
  let bufferTimer: ReturnType<typeof setTimeout> | undefined;
  let lastPublishedRequestId: string | null = null;

  const publish = (delivery: ControlDelivery) => {
    if (delivery.requestId === lastPublishedRequestId) return;
    lastPublishedRequestId = delivery.requestId;
    buffered = delivery;
    for (const subscriber of subscribers) subscriber(delivery);

    if (bufferTimer != null) clearTimeout(bufferTimer);
    bufferTimer = setTimeout(() => {
      if (buffered?.requestId === delivery.requestId) buffered = null;
      bufferTimer = undefined;
    }, 0);
  };

  const drain = () => {
    serializedDrain = serializedDrain.then(async () => {
      try {
        const deliveries = await drainNative();
        if (disposed || deliveries.length === 0) return;
        publish(deliveries[deliveries.length - 1]);
      } catch {
        // The optional control surface must never block ordinary tray use,
        // and a failed drain must not poison the serialized promise chain.
      }
    });
    return serializedDrain;
  };

  const start = () => {
    if (started || disposed) return;
    started = true;
    void (async () => {
      try {
        const stop = await listenNative(() => void drain());
        if (disposed) stop();
        else unlisten = stop;
      } catch {
        // The direct drain remains useful even if event registration fails.
      }
      if (disposed) return;
      await drain();
    })();
  };

  return {
    subscribe(subscriber) {
      if (disposed) return () => undefined;
      subscribers.add(subscriber);
      if (buffered != null) subscriber(buffered);
      start();
      return () => subscribers.delete(subscriber);
    },
    dispose() {
      disposed = true;
      subscribers.clear();
      unlisten?.();
      if (bufferTimer != null) clearTimeout(bufferTimer);
      buffered = null;
    },
  };
}

const deliveryBroker = createControlDeliveryBroker(
  takePendingControlDeliveries,
  onControlPending,
);

if (import.meta.hot) {
  import.meta.hot.dispose(() => deliveryBroker.dispose());
}

/** Subscribe the app to the process-level control broker. The callback ref
 * keeps React renders out of the native listener lifecycle. */
export function useControlDelivery(
  applyDelivery: (delivery: ControlDelivery) => void,
): void {
  const applyRef = useRef(applyDelivery);
  applyRef.current = applyDelivery;
  const lastAppliedRequestRef = useRef<string | null>(null);

  useEffect(
    () =>
      deliveryBroker.subscribe((delivery) => {
        if (delivery.requestId === lastAppliedRequestRef.current) return;
        lastAppliedRequestRef.current = delivery.requestId;
        applyRef.current(delivery);
      }),
    [],
  );
}
