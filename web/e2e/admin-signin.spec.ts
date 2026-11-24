// Admin › Sign-in & devices, as the owner.
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

test("the owner changes how long sessions last", async ({ page }) => {
  await page.goto("/admin/sign-in");
  await expect(page.getByRole("heading", { level: 1, name: en["admin.signIn"] })).toBeVisible();
  await expect(page.getByRole("switch", { name: en["policy.passkey"] })).toBeChecked();
  await page.getByLabel(en["policy.idle"]).selectOption("7");
  await expect(page.getByRole("status")).toHaveText(en["policy.saved"]);
  await page.reload();
  await expect(page.getByLabel(en["policy.idle"])).toHaveValue("7");
  await page.getByLabel(en["policy.idle"]).selectOption("14");
  await expect(page.getByRole("status")).toHaveText(en["policy.saved"]);
  await expect(page.getByRole("heading", { name: en["policy.recent"] })).toBeVisible();
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the sign-in policy page is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/admin/sign-in");
    await expect(page.getByRole("heading", { level: 1, name: catalog["admin.signIn"] as string })).toBeVisible();
    await expect(page.getByRole("heading", { name: catalog["policy.methods"] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}

test("screenshot of the sign-in policy for comparing with the board", async ({ page }) => {
  await page.goto("/admin/sign-in");
  await expect(page.getByRole("heading", { name: en["policy.recent"] })).toBeVisible();
  await page.waitForTimeout(400); // the switches finish moving
  await page.screenshot({ path: "test-results/admin-signin.png" });
});
