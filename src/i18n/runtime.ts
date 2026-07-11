import { i18n } from "@lingui/core";

export const APP_LOCALES = ["en", "ko", "ja"] as const;
export type AppLocale = (typeof APP_LOCALES)[number];
export type LocalePreference = "system" | AppLocale;

const catalogLoaders: Record<
  AppLocale,
  () => Promise<{ messages: Parameters<typeof i18n.load>[1] }>
> = {
  en: () => import("../locales/en/messages.po"),
  ko: () => import("../locales/ko/messages.po"),
  ja: () => import("../locales/ja/messages.po"),
};

let activationSequence = 0;

/** Load only the active catalog and publish it atomically. The sequence fence
 * prevents a slower previous import from winning when the user changes the
 * language twice in quick succession. */
export async function activateLocale(locale: AppLocale): Promise<void> {
  const sequence = ++activationSequence;
  let activeLocale = locale;
  let messages: Parameters<typeof i18n.load>[1];
  try {
    ({ messages } = await catalogLoaders[locale]());
  } catch (error) {
    if (locale === "en") throw error;
    // A damaged optional catalog must never strand the application before it
    // can render Settings. English is compiled into its own known-good chunk.
    activeLocale = "en";
    ({ messages } = await catalogLoaders.en());
  }
  if (sequence !== activationSequence) return;
  i18n.loadAndActivate({ locale: activeLocale, messages });
  document.documentElement.lang = activeLocale;
}

export { i18n };
