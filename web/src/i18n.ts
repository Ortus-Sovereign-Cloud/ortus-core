// Languages and text direction.
//
// Ortus text and the lit-material components' own text go through one
// mechanism: catalogs registered with lit-material's localize module, read
// by each component for the nearest `lang` attribute. Changing the language
// sets `lang` and `dir` on <html>; every component re-renders.
//
// Adding a language: add its catalog under locales/, and one entry below.
// A right-to-left language needs only `dir: "rtl"`; layout uses logical
// CSS properties and mirrors by itself.

import { LocalizeController, registerTranslation, type Message, type MessageArgs, type Translation } from "@lit-material/core/localize.js";
import en, { type MessageKey, type Messages } from "./locales/en.js";

export interface Locale {
  /** BCP 47 language tag. */
  code: string;
  /** The language's own name for itself. */
  name: string;
  dir: "ltr" | "rtl";
  /** Loads the Ortus catalog and the component catalog for the language. */
  load: () => Promise<[Messages, Translation | undefined]>;
}

export const LOCALES: readonly Locale[] = [
  { code: "en", name: "English", dir: "ltr", load: async () => [en, undefined] },
  { code: "pl", name: "Polski", dir: "ltr", load: async () => [(await import("./locales/pl.js")).default, (await import("@lit-material/translations/pl.js")).default] },
  { code: "uk", name: "Українська", dir: "ltr", load: async () => [(await import("./locales/uk.js")).default, (await import("@lit-material/translations/uk.js")).default] },
  { code: "de", name: "Deutsch", dir: "ltr", load: async () => [(await import("./locales/de.js")).default, (await import("@lit-material/translations/de.js")).default] },
  { code: "fr", name: "Français", dir: "ltr", load: async () => [(await import("./locales/fr.js")).default, (await import("@lit-material/translations/fr.js")).default] },
  { code: "ar", name: "العربية", dir: "rtl", load: async () => [(await import("./locales/ar.js")).default, (await import("@lit-material/translations/ar.js")).default] },
];

export const DEFAULT_LOCALE = "en";
const STORAGE_KEY = "ortus.locale";
/** Ortus keys live under this prefix in the shared catalog. */
const PREFIX = "ortus.";

export function findLocale(code: string): Locale | undefined {
  const wanted = code.toLowerCase();
  return LOCALES.find((locale) => locale.code === wanted) ?? LOCALES.find((locale) => locale.code === wanted.split("-")[0]);
}

/**
 * The language to start in: the person's saved choice, else the first of
 * the browser's languages that Ortus has, else English.
 */
export function pickLocale(saved: string | null | undefined, browser: readonly string[]): string {
  for (const candidate of [saved, ...browser]) {
    const found = candidate ? findLocale(candidate) : undefined;
    if (found) return found.code;
  }
  return DEFAULT_LOCALE;
}

export function savedLocale(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null; // storage can be blocked; the choice then lasts for the visit
  }
}

/** Switches the page to `code`: loads its catalogs, then sets `lang` and `dir`. */
export async function setLocale(code: string, remember = true): Promise<string> {
  const locale = findLocale(code) ?? findLocale(DEFAULT_LOCALE)!;
  const [messages, components] = await locale.load();
  const catalog: Translation = { $code: locale.code, $name: locale.name, $dir: locale.dir };
  for (const [key, message] of Object.entries(messages)) catalog[PREFIX + key] = message;
  registerTranslation(...(components ? [components, catalog] : [catalog]));
  document.documentElement.lang = locale.code;
  document.documentElement.dir = locale.dir;
  if (remember) {
    try {
      localStorage.setItem(STORAGE_KEY, locale.code);
    } catch {
      // see savedLocale
    }
  }
  return locale.code;
}

/** Sets the language the page starts in. */
export function startLocale(): Promise<string> {
  return setLocale(pickLocale(savedLocale(), navigator.languages ?? [navigator.language]), false);
}

/** The text for `key` in the component's language; English until a catalog loads. */
export function t(localize: LocalizeController, key: MessageKey, args?: MessageArgs): string {
  return localize.term(PREFIX + key, en[key] as Message, args);
}
