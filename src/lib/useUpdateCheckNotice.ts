import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

import type { UpdateCheckNotice } from "../types";
import {
  onUpdateCheckError,
  onUpdateChecking,
  onUpdateNone,
  onUpdateShowModal,
} from "./ipc";

/** Own the short-lived chrome feedback for explicit update checks.
 *
 * One timer owns all transitions, so a previous "up to date" decay cannot
 * erase a newer retry or error. Listener setup and teardown live here rather
 * than adding another concern to the already-large panel orchestrator. */
export function useUpdateCheckNotice(): UpdateCheckNotice | null {
  const [notice, setNotice] = useState<UpdateCheckNotice | null>(null);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    let mounted = true;
    let unChecking: UnlistenFn | undefined;
    let unNone: UnlistenFn | undefined;
    let unError: UnlistenFn | undefined;
    let unModal: UnlistenFn | undefined;

    const clear = () => {
      if (!mounted) return;
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      setNotice(null);
    };

    const show = (next: UpdateCheckNotice, timeoutMs: number) => {
      if (!mounted) return;
      if (timerRef.current != null) window.clearTimeout(timerRef.current);
      setNotice(next);
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null;
        if (mounted) setNotice(null);
      }, timeoutMs);
    };

    void (async () => {
      const [checking, none, error, modal] = await Promise.all([
        onUpdateChecking(() => show({ kind: "checking" }, 30_000)).catch(
          () => undefined,
        ),
        onUpdateNone(() => show({ kind: "current" }, 3_000)).catch(
          () => undefined,
        ),
        onUpdateCheckError((detail) =>
          show({ kind: "error", detail }, 8_000),
        ).catch(() => undefined),
        onUpdateShowModal(clear).catch(() => undefined),
      ]);
      if (!mounted) {
        checking?.();
        none?.();
        error?.();
        modal?.();
        return;
      }
      unChecking = checking;
      unNone = none;
      unError = error;
      unModal = modal;
    })();

    return () => {
      mounted = false;
      unChecking?.();
      unNone?.();
      unError?.();
      unModal?.();
      if (timerRef.current != null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, []);

  return notice;
}
