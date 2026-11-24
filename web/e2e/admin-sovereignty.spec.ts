// Admin › Sovereignty & compliance, as the owner.
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

test.describe.configure({ mode: "serial" });

test("the owner sees where data lives and who holds the keys", async ({ page }) => {
  await page.goto("/admin/sovereignty");
  await expect(page.getByRole("heading", { level: 1, name: en["admin.sovereignty"] })).toBeVisible();
  await expect(page.getByRole("region", { name: en["sovereignty.keys"] })).toContainText("AES-256-GCM");
  await expect(page.getByRole("region", { name: en["sovereignty.dataLocation"] })).toContainText("SQLite");
  await expect(page.getByRole("region", { name: en["sovereignty.labels"] }).getByRole("button")).toHaveCount(4);
  await expect(page.getByRole("button", { name: /^SECRET: / })).toContainText(en["sovereignty.reachInside"]);
});

test("the owner allows a partner, changes a label and a period, and places and releases a hold", async ({ page }) => {
  await page.goto("/admin/sovereignty");
  await page.getByRole("button", { name: en["sovereignty.allow"] }).click();
  await page.getByLabel(en["sovereignty.domain"]).fill("health.example.gov");
  await page.getByLabel(en["people.name"], { exact: true }).fill("Health");
  await page.getByRole("checkbox", { name: en["sovereignty.channel.chat"] }).check();
  await page.getByRole("button", { name: en["sovereignty.allowShort"], exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(en["sovereignty.partnerAdded"]);
  const federation = page.getByRole("region", { name: en["sovereignty.federation"] });
  await expect(federation).toContainText("health.example.gov");
  await expect(federation).toContainText(`${en["sovereignty.channel.mail"]} + ${en["sovereignty.channel.chat"]}`);

  await page.getByRole("button", { name: /^SECRET: / }).click();
  await page.getByLabel(en["sovereignty.rule.partners"]).selectOption("warn");
  await page.getByRole("button", { name: en["sovereignty.save"] }).click();
  await expect(page.getByRole("button", { name: /^SECRET: / })).toContainText(en["sovereignty.reachPartners"]);
  await page.getByRole("button", { name: /^SECRET: / }).click();
  await page.getByLabel(en["sovereignty.rule.partners"]).selectOption("deny");
  await page.getByRole("button", { name: en["sovereignty.save"] }).click();
  await expect(page.getByRole("button", { name: /^SECRET: / })).toContainText(en["sovereignty.reachInside"]);

  await page.getByLabel(en["sovereignty.kind.chat"]).selectOption(String(365));
  await expect(page.getByRole("status")).toHaveText(en["people.saved"]);

  await page.getByRole("button", { name: en["sovereignty.placeHold"] }).click();
  await page.getByLabel(en["sovereignty.case"]).fill("2026-114");
  await page.getByLabel(en["sovereignty.holdPeople"]).fill("taras@ministry.example");
  await page.getByRole("button", { name: en["sovereignty.place"], exact: true }).click();
  await expect(page.getByText(/2026-114/)).toBeVisible();
  await page.getByRole("button", { name: en["sovereignty.release"] }).click();
  await expect(page.getByText(/2026-114/)).toHaveCount(0);

  await federation.getByRole("button", { name: en["sovereignty.removePartner"].replace("{domain}", "health.example.gov") }).click();
  await expect(federation).not.toContainText("health.example.gov");
});

test("the compliance report downloads, and the audit chain is checked", async ({ page }) => {
  await page.goto("/admin/sovereignty");
  const download = page.waitForEvent("download");
  await page.getByRole("link", { name: en["sovereignty.export"] }).click();
  expect((await download).suggestedFilename()).toBe("ortus-compliance-report.json");
  await page.getByRole("button", { name: en["sovereignty.checkNow"] }).click();
  await expect(page.getByRole("status")).toHaveText(en["sovereignty.checked"]);
  await expect(page.getByText(/last check .* OK/)).toBeVisible();
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the sovereignty page is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/admin/sovereignty");
    await expect(page.getByRole("heading", { level: 1, name: catalog["admin.sovereignty"] as string })).toBeVisible();
    await expect(page.getByRole("heading", { name: catalog["sovereignty.labels"] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}

test("screenshot of the sovereignty page for comparing with the board", async ({ page }) => {
  await page.goto("/admin/sovereignty");
  await expect(page.getByRole("heading", { name: en["sovereignty.labels"] })).toBeVisible();
  await page.screenshot({ path: "test-results/admin-sovereignty.png" });
});
