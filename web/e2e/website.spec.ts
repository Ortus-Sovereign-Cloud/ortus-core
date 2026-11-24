// The project website at the root, for visitors; the app for people who
// are signed in.
import { expect, test } from "@playwright/test";
import { LOCALES } from "../src/i18n.js";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import es from "../src/locales/es.js";
import fr from "../src/locales/fr.js";
import it from "../src/locales/it.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

const CATALOGS = { en, pl, uk, de, fr, es, it, ar } as const;
type Code = keyof typeof CATALOGS;

test("someone signed in gets the app at the root, not the website", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("ortus-shell")).toBeAttached();
  await expect(page.locator("ortus-website")).toHaveCount(0);
});

test.describe("a visitor", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  for (const locale of LOCALES) {
    const catalog = CATALOGS[locale.code as Code];
    test(`sees the website in ${locale.name}`, async ({ page }) => {
      const errors: string[] = [];
      page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
      await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1, name: catalog["site.headline"] as string })).toBeVisible();
      await expect(page.getByRole("heading", { level: 2, name: catalog["site.faqTitle"] as string })).toBeAttached();
      await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
      expect(errors).toEqual([]);
    });
  }

  test("goes from the website to signing in", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("navigation").getByRole("link", { name: en["site.signIn"] }).click();
    await expect(page).toHaveURL(/\/sign-in$/);
  });

  test("fits a phone without sideways scrolling", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    const wide = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    expect(wide).toBe(false);
  });
});
