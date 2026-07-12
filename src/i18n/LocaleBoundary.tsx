import { useEffect, type ReactNode } from "react";
import { I18nProvider } from "@lingui/react";

import { useSettingsSnapshot } from "../lib/settings";
import { activateLocale, i18n } from "./runtime";

interface LocaleBoundaryProps {
  children: ReactNode;
}

/** Keeps each persistent Tauri webview on the backend-resolved locale.
 * Catalog activation causes I18nProvider to re-render translated consumers;
 * no window reload or duplicate settings listener is needed. */
export function LocaleBoundary({ children }: LocaleBoundaryProps) {
  const { resolvedLocale } = useSettingsSnapshot();

  useEffect(() => {
    if (i18n.locale === resolvedLocale) return;
    void activateLocale(resolvedLocale).catch((error: unknown) => {
      console.error("Failed to activate the application locale", error);
    });
  }, [resolvedLocale]);

  return <I18nProvider i18n={i18n}>{children}</I18nProvider>;
}
