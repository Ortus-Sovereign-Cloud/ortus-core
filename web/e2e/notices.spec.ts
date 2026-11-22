// The pages the app stores link to: the apps' privacy policy and support,
// readable signed out, in every language (Arabic right to left).
import { expect, test } from "@playwright/test";
import ar from "../src/locales/ar.js";
import en from "../src/locales/en.js";

test.use({ storageState: { cookies: [], origins: [] } });

for (const [path, title, section] of [
  ["/privacy", "privacy.title", "privacy.collectTitle"],
  ["/support", "support.title", "support.contactTitle"],
] as const) {
  test(`${path} is readable signed out`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.goto(path);
    await expect(page).toHaveURL(new RegExp(`${path}$`));
    await expect(page.getByRole("heading", { level: 1, name: en[title] as string })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: en[section] as string })).toBeVisible();
    await expect(page.getByRole("link", { name: "support@ortus.cc" })).toHaveAttribute("href", "mailto:support@ortus.cc");
    expect(errors).toEqual([]);

    await page.addInitScript(() => localStorage.setItem("ortus.locale", "ar"));
    await page.goto(path);
    await expect(page.getByRole("heading", { level: 1, name: ar[title] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  });
}
