import { useCallback, useEffect, useState, type Dispatch } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useLingui } from "@lingui/react";

import { explicitAddRepo, listRepos, setPanelSticky } from "../lib/ipc";
import type { RepoCatalogAction } from "../lib/repoCatalog";
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

  const addPath = useCallback(
    async (rawPath: string): Promise<boolean> => {
      const trimmed = rawPath.trim();
      if (!trimmed) return false;
      try {
        const repo = await explicitAddRepo(trimmed);
        // The discovery event can beat listener registration. Publish the
        // accepted row immediately, then replace it with its persisted id.
        dispatchCatalog({ type: "discovered", repo });
        void listRepos()
          .then((repos) => dispatchCatalog({ type: "replace", repos }))
          .catch(() => {});
        setAddError(null);
        return true;
      } catch (error) {
        setAddError(
          typeof error === "string"
            ? error
            : error instanceof Error
              ? error.message
              : i18n._(panelMessages.addRepoFailed),
        );
        window.setTimeout(() => setAddError(null), 4_000);
        return false;
      }
    },
    [dispatchCatalog, i18n],
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
      for (const path of Array.isArray(selected) ? selected : [selected]) {
        await addPath(path);
      }
    } catch {
      // Drop/paste remain available if the platform picker cannot open.
    } finally {
      setDialogOpen(false);
    }
  }, [addPath, i18n]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void getCurrentWindow()
      .listen<{ type: string; paths?: string[] }>("tauri://drag-drop", (event) => {
        if (event.payload.type !== "drop") return;
        for (const path of event.payload.paths ?? []) void addPath(path);
      })
      .then((stop) => {
        unlisten = stop;
      });
    return () => unlisten?.();
  }, [addPath]);

  useEffect(() => {
    function onPaste(event: ClipboardEvent) {
      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.getAttribute("contenteditable") === "true")
      ) {
        return;
      }
      const text = event.clipboardData?.getData("text/plain")?.trim() ?? "";
      if (!/^([a-zA-Z]:[\\/]|\/|~[\\/])/.test(text)) return;
      event.preventDefault();
      void addPath(text);
    }
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [addPath]);

  return { addError, browse };
}
