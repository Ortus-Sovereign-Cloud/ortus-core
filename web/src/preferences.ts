// One's own preferences: kept on the server, applied to the page as
// attributes on <html> (styles.css reads them; no style attributes, which
// the Content-Security-Policy refuses).
import { call } from "./api.js";
import { savedLocale, setLocale } from "./i18n.js";

export interface Preferences {
  /** null: the browser's language. */
  language: string | null;
  text_size: 100 | 110 | 125 | 150;
  motion: "system" | "reduce";
}

export const TEXT_SIZES = [100, 110, 125, 150] as const;

export function applyPreferences(preferences: Preferences): void {
  const root = document.documentElement;
  root.dataset.textSize = String(preferences.text_size);
  root.dataset.motion = preferences.motion;
}

export async function loadPreferences(): Promise<Preferences> {
  return call<Preferences>("GET", "/me/preferences");
}

export async function savePreferences(change: Partial<Preferences>): Promise<Preferences> {
  const saved = await call<Preferences>("PUT", "/me/preferences", change);
  applyPreferences(saved);
  return saved;
}

/**
 * After signing in: the saved preferences take effect. The account's
 * language applies on a device that has not chosen one of its own (the
 * rail's language menu chooses for this device only).
 */
export async function startPreferences(): Promise<void> {
  const preferences = await loadPreferences();
  applyPreferences(preferences);
  if (preferences.language && !savedLocale() && preferences.language !== document.documentElement.lang) await setLocale(preferences.language);
}
