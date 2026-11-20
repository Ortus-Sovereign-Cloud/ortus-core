import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { formatMessage, registerTranslation, type Message, type Translation } from "@lit-material/core/localize.js";
import { DEFAULT_LOCALE, LOCALES, findLocale, pickLocale } from "../src/i18n.js";
import en from "../src/locales/en.js";

const src = join(import.meta.dirname, "../src");

async function catalogs(): Promise<Map<string, Record<string, Message>>> {
  const loaded = new Map<string, Record<string, Message>>();
  for (const locale of LOCALES) loaded.set(locale.code, (await locale.load())[0]);
  return loaded;
}

function placeholders(message: Message): string[] {
  const texts = typeof message === "string" ? [message] : Object.values(message);
  return [...new Set(texts.flatMap((text) => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]!)))].sort();
}

describe("languages", () => {
  it("offers English, Polish, Ukrainian, German, French and Arabic", () => {
    expect(LOCALES.map((locale) => locale.code)).toEqual(["en", "pl", "uk", "de", "fr", "ar"]);
    expect(LOCALES.filter((locale) => locale.dir === "rtl").map((locale) => locale.code)).toEqual(["ar"]);
  });

  it("has a catalog file for every language and no stray ones", () => {
    const files = readdirSync(join(src, "locales")).map((file) => file.replace(/\.ts$/, "")).sort();
    expect(files).toEqual(LOCALES.map((locale) => locale.code).sort());
  });

  it("translates every message in every language, with nothing extra", async () => {
    const keys = Object.keys(en).sort();
    for (const [code, catalog] of await catalogs()) {
      expect(Object.keys(catalog).sort(), code).toEqual(keys);
      for (const key of keys) {
        const message = catalog[key]!;
        const texts = typeof message === "string" ? [message] : Object.values(message);
        for (const text of texts) expect(text.trim(), `${code} ${key}`).not.toBe("");
      }
    }
  });

  it("keeps the same placeholders as English", async () => {
    for (const [code, catalog] of await catalogs()) {
      for (const [key, message] of Object.entries(en)) {
        const expected = placeholders(message);
        const actual = placeholders(catalog[key]!);
        // A plural form may spell a number out ("one app"), so a
        // translation may drop {count} from some forms but never add
        // a placeholder English does not pass.
        expect(actual.every((name) => expected.includes(name)), `${code} ${key}: ${actual} vs ${expected}`).toBe(true);
        if (typeof message === "string") expect(actual, `${code} ${key}`).toEqual(expected);
      }
    }
  });

  it("gives every plural message each form its language needs", async () => {
    for (const [code, catalog] of await catalogs()) {
      const needed = new Intl.PluralRules(code).resolvedOptions().pluralCategories;
      for (const [key, message] of Object.entries(catalog)) {
        if (typeof message === "string") continue;
        expect(typeof en[key as keyof typeof en], `${code} ${key} is plural but English is not`).toBe("object");
        for (const category of needed) expect(message[category], `${code} ${key} lacks the "${category}" form`).toBeTruthy();
      }
    }
  });

  it("chooses the right plural form for a count", async () => {
    const expected: Record<string, [number, string][]> = {
      en: [[1, "1 app"], [2, "2 apps"], [0, "0 apps"]],
      pl: [[1, "1 aplikacja"], [2, "2 aplikacje"], [5, "5 aplikacji"], [22, "22 aplikacje"], [25, "25 aplikacji"]],
      uk: [[1, "1 застосунок"], [3, "3 застосунки"], [5, "5 застосунків"], [21, "21 застосунок"], [11, "11 застосунків"]],
      de: [[1, "1 App"], [7, "7 Apps"]],
      fr: [[0, "0 application"], [1, "1 application"], [2, "2 applications"]],
      ar: [[0, "لا تطبيقات"], [1, "تطبيق واحد"], [2, "تطبيقان"], [3, "3 تطبيقات"], [11, "11 تطبيقًا"], [100, "100 تطبيق"]],
    };
    for (const [code, catalog] of await catalogs()) {
      const translation: Translation = { $code: code };
      for (const [key, message] of Object.entries(catalog)) translation[`ortus.${key}`] = message;
      registerTranslation(translation);
      for (const [count, text] of expected[code]!) {
        expect(formatMessage(code, "ortus.switcher.count", en["switcher.count"], { count }), `${code} ${count}`).toBe(text);
      }
    }
  });

  it("starts in the saved language, else the browser's, else English", () => {
    expect(pickLocale("uk", ["de-DE", "en"])).toBe("uk");
    expect(pickLocale(null, ["de-DE", "en"])).toBe("de");
    expect(pickLocale(null, ["ja", "ar-EG", "en"])).toBe("ar");
    expect(pickLocale("xx", ["ja"])).toBe(DEFAULT_LOCALE);
    expect(pickLocale(undefined, [])).toBe(DEFAULT_LOCALE);
    expect(findLocale("PL-pl")?.code).toBe("pl");
    expect(findLocale("zz")).toBeUndefined();
  });
});

describe("right-to-left layout", () => {
  // Physical properties (left, right) do not mirror. Styles must use the
  // logical ones (inline-start, inline-end) so every RTL language works.
  const physical = /(^|[\s;{])((margin|padding|border)-(left|right)|left|right)\s*:|text-align\s*:\s*(left|right)|float\s*:\s*(left|right)/;

  function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? sources(join(dir, entry.name)) : [join(dir, entry.name)]));
  }

  it("uses no physical left or right in any style", () => {
    const offenders: string[] = [];
    for (const file of sources(src).filter((file) => /\.(ts|css)$/.test(file) && !file.includes("locales"))) {
      readFileSync(file, "utf8")
        .split("\n")
        .forEach((line, index) => {
          if (physical.test(line)) offenders.push(`${file.slice(src.length + 1)}:${index + 1}: ${line.trim()}`);
        });
    }
    expect(offenders).toEqual([]);
  });

  // The Content-Security-Policy has no 'unsafe-inline' for styles, so a
  // style attribute in a template is silently ignored by the browser.
  // Even styleMap writes the attribute on its first render, so dynamic
  // values use elements made for them (progress, meter) or classes.
  it("puts no style attribute in any template", () => {
    const offenders: string[] = [];
    for (const file of sources(src).filter((file) => file.endsWith(".ts") && !file.includes("locales"))) {
      readFileSync(file, "utf8")
        .split("\n")
        .forEach((line, index) => {
          if (/\sstyle=/.test(line)) offenders.push(`${file.slice(src.length + 1)}:${index + 1}: ${line.trim()}`);
        });
    }
    expect(offenders).toEqual([]);
  });

  it("would catch a physical property", () => {
    for (const line of ["  margin-left: 4px;", "left: 0;", "{ right: 12px }", "text-align: right;", "border-right: 1px solid red;"]) expect(physical.test(line), line).toBe(true);
    for (const line of ["margin-inline-start: 4px;", "inset-inline-end: 0;", "text-align: start;", "// the right form", "copyright: none"]) expect(physical.test(line), line).toBe(false);
  });
});
