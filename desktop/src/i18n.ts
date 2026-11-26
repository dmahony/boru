import english from "../../src/bin/boru/locales/en.json";
import french from "../../src/bin/boru/locales/fr.json";

type Locale = Record<string, string>;
const dictionaries: Record<string, Locale> = { en: english as Locale, fr: french as Locale };

/** Translate using the same key dictionaries as the Iced frontend. */
export function t(key: string, args: Record<string, string | number> = {}): string {
  const language = (navigator.language || "en").split("-")[0].toLowerCase();
  let value = dictionaries[language]?.[key] ?? (english as Locale)[key] ?? key;
  for (const [name, replacement] of Object.entries(args)) {
    value = value.split(`{${name}}`).join(String(replacement));
  }
  return value;
}

export function localeDirection(_language = navigator.language): "ltr" {
  // Only English and French are currently supported; both are left-to-right.
  return "ltr";
}
