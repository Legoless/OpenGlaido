// App language. Strings are written in English and used as their own keys: t("Retry").
// Translations live in src/locales/<code>.json ({"Retry": "Wiederholen", ...}); a missing key
// falls back to English. `{name}` placeholders are filled from `vars`.
import de from "./locales/de.json";
import el from "./locales/el.json";
import es from "./locales/es.json";
import fr from "./locales/fr.json";
import it from "./locales/it.json";
import ja from "./locales/ja.json";
import ko from "./locales/ko.json";
import nl from "./locales/nl.json";
import pl from "./locales/pl.json";
import ro from "./locales/ro.json";
import ru from "./locales/ru.json";
import srLatn from "./locales/sr-Latn.json";
import zhHans from "./locales/zh-Hans.json";

/** Options for Settings › General › App language, each written in its own language. */
export const LOCALES: { code: string; label: string }[] = [
  { code: "en", label: "English" },
  { code: "de", label: "Deutsch" },
  { code: "es", label: "Español" },
  { code: "nl", label: "Nederlands" },
  { code: "fr", label: "Français" },
  { code: "it", label: "Italiano" },
  { code: "pl", label: "Polski" },
  { code: "ro", label: "Română" },
  { code: "ru", label: "Русский" },
  { code: "sr-Latn", label: "Srpski (latinica)" },
  { code: "el", label: "Ελληνικά" },
  { code: "ja", label: "日本語" },
  { code: "ko", label: "한국어" },
  { code: "zh-Hans", label: "简体中文" },
];

export const DICTIONARIES: Record<string, Record<string, string>> = {
  de, es, nl, fr, it, pl, ro, ru, "sr-Latn": srLatn, el, ja, ko, "zh-Hans": zhHans,
};

/** "system" follows the OS language (navigator.languages), falling back to English. */
export function resolveLocale(pref: string | undefined): string {
  if (pref && pref !== "system") return LOCALES.some((l) => l.code === pref) ? pref : "en";
  for (const tag of navigator.languages ?? [navigator.language]) {
    const lower = tag.toLowerCase();
    if (lower.startsWith("zh")) return "zh-Hans";
    if (lower.startsWith("sr")) return "sr-Latn";
    const base = lower.split("-")[0];
    if (LOCALES.some((l) => l.code === base)) return base;
  }
  return "en";
}

let current = "en";

/** Set before rendering (App does it from config.app_language). */
export function setLocale(code: string) {
  current = code;
  document.documentElement.lang = code;
}

export function locale(): string {
  return current;
}

export function t(text: string, vars?: Record<string, string | number>): string {
  const translated = DICTIONARIES[current]?.[text] || text;
  if (!vars) return translated;
  return translated.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
}
