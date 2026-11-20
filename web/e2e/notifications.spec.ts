// The bell and the notification centre (WebNotifications board) in six
// languages: the panel opens from the rail with its tabs and quiet
// hours; do not disturb is kept.
import { expect, test } from "@playwright/test";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import fr from "../src/locales/fr.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

for (const [code, texts, dir] of [["en", en, "ltr"], ["pl", pl, "ltr"], ["uk", uk, "ltr"], ["de", de, "ltr"], ["fr", fr, "ltr"], ["ar", ar, "rtl"]] as const) {
  test(`the notification centre opens in ${code}`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.addInitScript((c) => localStorage.setItem("ortus.locale", c), code);
    await page.goto("/settings");
    await page.getByRole("button", { name: texts["notify.open"] as string }).click();
    const panel = page.getByRole("dialog", { name: texts["notify.title"] as string });
    await expect(panel).toBeVisible();
    await expect(panel.getByRole("button", { name: texts["notify.alerts"] as string })).toBeVisible();
    await expect(panel.getByText(texts["notify.none"] as string)).toBeVisible();
    await expect(panel.getByText(texts["notify.noQuiet"] as string)).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", dir);
    expect(errors).toEqual([]);
  });
}

test("do not disturb is kept", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: en["notify.open"] }).click();
  const panel = page.getByRole("dialog", { name: en["notify.title"] });
  const toggle = panel.getByRole("switch", { name: en["notify.dnd"] });
  const was = await toggle.isChecked();
  await toggle.click();
  await expect(toggle).toBeChecked({ checked: !was });
  await page.reload();
  await page.getByRole("button", { name: en["notify.open"] }).click();
  await expect(page.getByRole("dialog", { name: en["notify.title"] }).getByRole("switch", { name: en["notify.dnd"] })).toBeChecked({ checked: !was });
});
