// Admin › Overview: the steps to go live, as the owner.
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

test("the overview shows six steps and the installation", async ({ page }) => {
  await page.goto("/admin");
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Taras");
  await expect(page.getByRole("listitem")).toHaveCount(6);
  await expect(page.locator("li[aria-current=step]")).toHaveCount(1);
  await expect(page.getByRole("heading", { name: en["overview.installation"] })).toBeVisible();
  await expect(page.getByText(en["overview.healthy"])).toBeVisible();
});

test("confirming where data lives marks that step done", async ({ page }) => {
  const answer = await page.request.post("/api/v1/admin/overview/data-confirmed", { headers: { "X-Requested-With": "ortus" } });
  expect(answer.status()).toBe(200);
  await page.goto("/admin");
  const step = page.getByRole("listitem").filter({ hasText: en["overview.data.title"] });
  await expect(step.getByText(en["overview.done"], { exact: true })).toBeVisible();
});

test("adding a second owner opens the invitation as an owner", async ({ page }) => {
  await page.goto("/admin");
  await page.getByRole("link", { name: en["overview.addOwner"] }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.locator("ortus-admin-users").locator("#invite-role")).toHaveValue("owner");
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the overview is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/admin");
    await expect(page.getByRole("heading", { name: catalog["overview.installation"] as string })).toBeVisible();
    await expect(page.getByText(catalog["overview.recovery_kit.title"] as string)).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}

test("screenshot of the overview for comparing with the board", async ({ page }) => {
  await page.goto("/admin");
  await expect(page.getByRole("heading", { name: en["overview.installation"] })).toBeVisible();
  await page.screenshot({ path: "test-results/admin-overview.png" });
});
