// WCAG 2.2 AA with axe on the core's screens: desktop and phone, light
// and dark, and the sign-in page signed out. The page's CSP forbids
// inline scripts; the test bypasses it to put axe in.
import { expect, test } from "@playwright/test";
import { axe } from "./axe.js";

test.use({ bypassCSP: true });

const PAGES = ["/", "/apps", "/settings", "/settings/security", "/admin", "/admin/users", "/admin/sign-in", "/admin/sovereignty", "/admin/backup", "/admin/integrations"];

for (const [name, size] of [["desktop", { width: 1440, height: 900 }], ["phone", { width: 390, height: 844 }]] as const) {
  for (const scheme of ["light", "dark"] as const) {
    test(`the core's screens pass axe: ${name}, ${scheme}`, async ({ page }) => {
      await page.setViewportSize(size);
      await page.emulateMedia({ colorScheme: scheme });
      const failures: string[] = [];
      for (const path of PAGES) {
        await page.goto(path);
        // Live streams keep the network busy: wait for the screen instead.
        await page.locator("main#content > *").first().waitFor();
        await page.waitForTimeout(500);
        failures.push(...(await axe(page)).map((f) => `${path}: ${f}`));
      }
      expect(failures).toEqual([]);
    });
  }
}

test("the sign-in page passes axe", async ({ browser }) => {
  const page = await (await browser.newContext({ storageState: { cookies: [], origins: [] }, bypassCSP: true })).newPage();
  await page.goto("/sign-in");
  await page.getByRole("button").first().waitFor();
  expect(await axe(page)).toEqual([]);
  await page.context().close();
});

test("axe is really looking: an image without a text fails", async ({ page }) => {
  await page.goto("/apps");
  await page.locator("main#content > *").first().waitFor();
  await page.evaluate(() => document.body.append(Object.assign(document.createElement("img"), { src: "/favicon.svg" })));
  expect((await axe(page)).join()).toContain("image-alt");
});
