import { useCallback, useEffect, useRef, useState } from "react";

import {
  getPanelWindowState,
  onPanelWindowState,
  togglePanelMaximized,
  type PanelWindowState,
} from "../lib/ipc";

const INITIAL_STATE: PanelWindowState = {
  maximized: false,
  chromeDoubleClickMaximize: false,
};

export interface PanelWindowController extends PanelWindowState {
  busy: boolean;
  toggleMaximized: () => Promise<void>;
}

/** Authoritative native panel geometry state. The event listener is installed
 * before the initial read so an OS maximize that lands during mount cannot be
 * overwritten by an older snapshot. */
export function usePanelWindowState(): PanelWindowController {
  const [state, setState] = useState<PanelWindowState>(INITIAL_STATE);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    let sawEvent = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      try {
        unlisten = await onPanelWindowState((next) => {
          sawEvent = true;
          if (!disposed) setState(next);
        });
        if (disposed) {
          unlisten();
          return;
        }
        const initial = await getPanelWindowState();
        if (!disposed && !sawEvent) setState(initial);
      } catch (error) {
        // The panel remains usable at normal size even if native state IPC is
        // unavailable; log the diagnostic instead of blanking the webview.
        // eslint-disable-next-line no-console
        console.error("[gitwink] panel window state initialization failed", error);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const toggleMaximized = useCallback(async () => {
    // State-driven `disabled` arrives on the next React render. The ref closes
    // the smaller same-frame window where a rapid double click could dispatch
    // maximize and restore concurrently.
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      setState(await togglePanelMaximized());
    } catch (error) {
      // eslint-disable-next-line no-console
      console.error("[gitwink] toggle panel maximize failed", error);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, []);

  return { ...state, busy, toggleMaximized };
}
