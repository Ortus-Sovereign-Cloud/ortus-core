// My settings › Mail apps: a password made for one app, shown once, removed.
import { expect, test } from "@playwright/test";
import en from "../src/locales/en.js";

test("an app password is made, shown once and removed", async ({ page }) => {
  await page.goto("/settings");
  const card = page.getByRole("region", { name: en["settings.appPasswords"] });
  await card.getByLabel(en["settings.appName"], { exact: true }).fill("Thunderbird on the laptop");
  await card.getByRole("button", { name: en["settings.makeAppPassword"] }).click();
  const shown = card.getByRole("status");
  await expect(shown).toContainText(en["settings.appPasswordMade"].replace("{name}", "Thunderbird on the laptop"));
  await expect(shown.locator("code")).toHaveText(/^[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}$/);
  await expect(card).toContainText(en["settings.neverUsed"]);

  // Gone from the page after a reload: shown once only.
  await page.reload();
  await expect(card.getByRole("status")).toHaveCount(0);
  await card.getByRole("button", { name: en["settings.removeAppPassword"].replace("{name}", "Thunderbird on the laptop") }).click();
  await expect(card).not.toContainText("Thunderbird on the laptop");
});
