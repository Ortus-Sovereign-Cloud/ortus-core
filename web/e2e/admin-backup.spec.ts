// Admin › Backup & restore: restore points, bringing one thing back into a
// new place, self-restore, the last restore test; in every language.
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

test.describe.configure({ mode: "serial" });

test("a restore point is taken and self-restore turns on and off", async ({ page }) => {
  await page.goto("/admin/backup");
  const screen = page.locator("ortus-admin-backup");
  await expect(screen.getByRole("heading", { level: 1 })).toHaveText(en["backup.title"]);
  const restore = screen.getByRole("region", { name: en["backup.restore"] });
  // The first point, taken on demand (the hourly one may be there already).
  const take = restore.getByRole("button", { name: en["backup.takeNow"] });
  if (await take.isVisible()) await take.click();
  await expect(restore.getByText(en["backup.legend"])).toBeVisible();
  await expect(restore.getByRole("group", { name: en["backup.asOf"] }).getByRole("button")).not.toHaveCount(0);
  // A restore test on demand: the point restored whole and checked.
  if (await restore.getByText(en["backup.noTest"]).isVisible()) {
    await restore.getByRole("button", { name: en["backup.testNow"] }).click();
    await expect(restore.getByText(/^Last restore test: .* · full site recovery in /)).toBeVisible({ timeout: 90_000 });
  }
  await restore.getByRole("button", { name: en["backup.selfOff"] }).click();
  await expect(restore.getByRole("button", { name: en["backup.selfOn"] })).toHaveAttribute("aria-pressed", "true");
  await restore.getByRole("button", { name: en["backup.selfOn"] }).click();
  await expect(restore.getByRole("button", { name: en["backup.selfOff"] })).toBeVisible();
});

test("one person's folder comes back into a new folder", async ({ page }) => {
  const at = Date.UTC(2026, 10, 17, 18, 0);
  await page.route("**/api/v1/admin/backup", (route) =>
    route.fulfill({
      json: {
        points: [
          { name: "20261117-180000", at, tables: 40, files: 120, bytes: 1 },
          { name: "20261117-170000", at: at - 3_600_000, tables: 40, files: 100, bytes: 1 },
        ],
        lastTest: { at: Date.UTC(2026, 10, 2, 3, 0), ok: true, durationMs: (3 * 60 + 40) * 60_000, detail: "" },
        selfRestore: true,
        restorable: [{ app: "mail", what: "folder" }, { app: "drive", what: "folder" }],
      },
    }),
  );
  await page.route("**/api/v1/backup/choices?**", (route) => route.fulfill({ json: { at, choices: [{ id: "mb_1", name: "Agreements" }] } }));
  let asked: Record<string, string> = {};
  await page.route("**/api/v1/backup/restore", (route) => {
    asked = route.request().postDataJSON() as Record<string, string>;
    return route.fulfill({ json: { name: "Agreements (2026-11-17 18:00)", path: "/mail?box=mb_9" } });
  });
  await page.goto("/admin/backup");
  const restore = page.locator("ortus-admin-backup").getByRole("region", { name: en["backup.restore"] });
  await expect(restore.getByRole("combobox", { name: en["backup.which"] })).toHaveValue("mb_1");
  await expect(restore.getByText("full site recovery in 3h 40m")).toBeVisible();
  await restore.getByRole("button", { name: en["backup.restoreButton"] }).click();
  await expect(restore.getByRole("status")).toContainText("Brought back as “Agreements (2026-11-17 18:00)”.");
  await expect(restore.getByRole("link", { name: en["backup.open"] })).toHaveAttribute("href", "/mail?box=mb_9");
  expect(asked).toMatchObject({ point: "20261117-180000", app: "mail", what: "folder", id: "mb_1" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: "test-results/admin-backup.png" });
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`backup and restore is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/admin/backup");
    const screen = page.locator("ortus-admin-backup");
    await expect(screen.getByRole("heading", { level: 1 })).toHaveText(catalog["backup.title"] as string);
    await expect(screen.getByRole("heading", { level: 2, name: catalog["backup.restore"] as string })).toBeVisible();
    await expect(page.getByRole("link", { name: catalog["admin.backup"] as string })).toHaveAttribute("aria-current", "page");
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}
