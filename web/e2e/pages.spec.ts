// The pages shown before sign-in, in every language.
import { expect, test } from "@playwright/test";
import { LOCALES } from "../src/i18n.js";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import fr from "../src/locales/fr.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

const CATALOGS = { en, pl, uk, de, fr, ar } as const;
type Code = keyof typeof CATALOGS;

test.describe("someone not signed in", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  for (const locale of LOCALES) {
    const catalog = CATALOGS[locale.code as Code];
    test(`is sent to the sign-in page, in ${locale.name}`, async ({ page }) => {
      await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
      await page.goto("/mail");
      await expect(page).toHaveURL(/\/sign-in$/);
      await expect(page.getByRole("heading", { level: 1, name: catalog["signIn.title"] as string })).toBeVisible();
      await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
    });

    test(`cannot set up an installation that has its owner, in ${locale.name}`, async ({ page }) => {
      await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
      await page.goto("/setup#e2e-setup-token");
      await page.getByLabel(catalog["setup.name"] as string).fill("Someone");
      await page.getByLabel(catalog["setup.email"] as string).fill("someone@example.org");
      await page.getByRole("button", { name: catalog["setup.create"] as string }).click();
      await expect(page.getByRole("alert")).toHaveText(catalog["setup.done"] as string);
    });
  }

  test("a setup page without its link says where to find it", async ({ page }) => {
    await page.goto("/setup");
    await expect(page.getByText(en["setup.noLink"])).toBeVisible();
    await expect(page.getByRole("button")).toHaveCount(0);
  });
});
