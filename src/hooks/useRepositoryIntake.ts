import { useCallback, useEffect, useRef, useState, type Dispatch } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useLingui } from "@lingui/react";

import { explicitAddRepo, listRepos, setPanelSticky } from "../lib/ipc";
import type { RepoCatalogAction } from "../lib/repoCatalog";
import {
  parseRepositoryPathClipboard,
  repositoryPasteShortcut,
} from "../lib/repositoryPathPaste";
import { panelMessages } from "../i18n/messages/panel";

interface RepositoryIntakeOptions {
  emptyState: boolean;
  dispatchCatalog: Dispatch<RepoCatalogAction>;
}

/** Owns every user-facing repository intake surface—folder picker, OS drop,
 * path paste, sticky-window lifetime, and transient failure feedback. Catalog
 * discovery itself remains in App's reducer; this hook publishes actions only
 * after the backend accepts a path. */
export function useRepositoryIntake({
  emptyState,
  dispatchCatalog,
}: RepositoryIntakeOptions) {
  const { i18n } = useLingui();
  const [addError, setAddError] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [pasting, setPasting] = useState(false);
  const [pasteShortcut] = useState(() => repositoryPasteShortcut());
  const errorTimerRef = useRef<number | null>(null);
  const pasteInFlightRef = useRef(false);

  const clearAddError = useCallback(() => {
    if (errorTimerRef.current != null) {
      window.clearTimeout(errorTimerRef.current);
      errorTimerRef.current = null;
    }
    setAddError(null);
  }, []);

  const showAddError = useCallback((message: string) => {
    if (errorTimerRef.current != null) {
      window.clearTimeout(errorTimerRef.current);
    }
    setAddError(message);
    errorTimerRef.current = window.setTimeout(() => {
      errorTimerRef.current = null;
      setAddError(null);
    }, 4_000);
  }, []);

  useEffect(
    () => () => {
      if (errorTimerRef.current != null) {
        window.clearTimeout(errorTimerRef.current);
      }
    },
    [],
  );

  const addPaths = useCallback(
    async (rawPaths: readonly string[]): Promise<boolean> => {
      const paths = rawPaths.map((path) => path.trim()).filter(Boolean);
      if (paths.length === 0) return false;

      let accepted = false;
      let lastError: string | null = null;
      for (const path of paths) {
        try {
          const repo = await explicitAddRepo(path);
          // The discovery event can beat listener registration. Publish the
          // accepted row immediately, then replace it with its persisted id.
          dispatchCatalog({ type: "discovered", repo });
          accepted = true;
        } catch (error) {
          lastError =
            typeof error === "string"
              ? error
              : error instanceof Error
                ? error.message
                : i18n._(panelMessages.addRepoFailed);
        }
      }

      if (accepted) {
        void listRepos()
          .then((repos) => dispatchCatalog({ type: "replace", repos }))
          .catch(() => {});
      }

      if (lastError) showAddError(lastError);
      else if (accepted) clearAddError();
      return accepted;
    },
    [clearAddError, dispatchCatalog, i18n, showAddError],
  );

  useEffect(() => {
    void setPanelSticky(emptyState || dialogOpen);
  }, [emptyState, dialogOpen]);

  const browse = useCallback(async () => {
    setDialogOpen(true);
    // Push sticky synchronously: the native picker can steal focus before the
    // effect above runs, which would otherwise hide the panel mid-flow.
    await setPanelSticky(true);
    try {
      const selected = await open({
        directory: true,
        multiple: true,
        title: i18n._(panelMessages.addRepoDialogTitle),
      });
      if (!selected) return;
      await addPaths(Array.isArray(selected) ? selected : [selected]);
    } catch {
      // Drop/paste remain available if the platform picker cannot open.
    } finally {
      setDialogOpen(false);
    }
  }, [addPaths, i18n]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void getCurrentWindow()
      .listen<{ type: string; paths?: string[] }>("tauri://drag-drop", (event) => {
        if (event.payload.type !== "drop") return;
        void addPaths(event.payload.paths ?? []);
      })
      .then((stop) => {
        unlisten = stop;
      });
    return () => unlisten?.();
  }, [addPaths]);

  useEffect(() => {
    function onPaste(event: ClipboardEvent) {
      if (
        event.target instanceof Element &&
        event.target.closest(
          'input, textarea, [contenteditable]:not([contenteditable="false"])',
        )
      ) {
        return;
      }
      const text = event.clipboardData?.getData("text/plain") ?? "";
      const paths = parseRepositoryPathClipboard(text);
      if (paths.length === 0) return;
      event.preventDefault();
      void addPaths(paths);
    }
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [addPaths]);

  const pasteFromClipboard = useCallback(async (): Promise<boolean> => {
    if (pasteInFlightRef.current) return false;
    pasteInFlightRef.current = true;
    setPasting(true);
    try {
      const paths = parseRepositoryPathClipboard(await readText());
      if (paths.length === 0) {
        showAddError(i18n._(panelMessages.pasteNoPath));
        return false;
      }
      return await addPaths(paths);
    } catch {
      showAddError(i18n._(panelMessages.pasteReadFailed));
      return false;
    } finally {
      pasteInFlightRef.current = false;
      setPasting(false);
    }
  }, [addPaths, i18n, showAddError]);

  return { addError, browse, pasteFromClipboard, pasteShortcut, pasting };
}
