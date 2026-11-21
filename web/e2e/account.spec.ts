// The account menu on the rail and a profile photo: a person of their own
// (invited here), so the shared owner's photo stays as it is.
import { expect, test, type Browser, type Page } from "@playwright/test";
import en from "../src/locales/en.js";

// A 40 by 30 PNG (not square: the page crops it).
const PHOTO = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAACgAAAAeCAIAAADRv8uKAAAAKklEQVR4nO3NMQ0AAAgDsAmb/yALGXA06d9MeyJisVgsFovFYrFYLP4bL9HP3Ew1mJ9PAAAAAElFTkSuQmCC", "base64");

async function newcomer(owner: Page, browser: Browser): Promise<{ page: Page; email: string }> {
  const email = `photo-${Date.now()}-${Math.floor(Math.random() * 1e6)}@ministry.example`;
  const invited = await owner.request.post("/api/v1/admin/users", { headers: { "X-Requested-With": "ortus" }, data: { email, name: "Iryna Savchenko" } });
  expect(invited.status()).toBe(200);
  const link = new URL(((await invited.json()) as { link: string }).link);
  const page = await (await browser.newContext({ storageState: { cookies: [], origins: [] } })).newPage();
  await page.goto(link.pathname + link.hash);
  await expect(page).toHaveURL(/\/welcome$/);
  return { page, email };
}

test("the account menu names the person and leads to their settings", async ({ page, browser }) => {
  const { page: iryna, email } = await newcomer(page, browser);
  await iryna.goto("/");
  await iryna.getByRole("button", { name: en["account.menu"] }).click();
  const menu = iryna.getByRole("menu", { name: en["account.menu"] });
  await expect(menu).toContainText("Iryna Savchenko");
  await expect(menu).toContainText(email);
  await expect(menu).toContainText(en["people.member"]);
  await menu.getByRole("menuitem", { name: en["account.settings"] }).click();
  await expect(iryna).toHaveURL(/\/settings$/);
});

test("a person adds a photo, sees it everywhere, and removes it", async ({ page, browser }) => {
  const { page: iryna } = await newcomer(page, browser);
  await iryna.goto("/settings");
  const profile = iryna.getByRole("region", { name: en["settings.profile"] });
  await expect(profile.locator("ortus-avatar img")).toHaveCount(0);
  await profile.locator("#photo-file").setInputFiles({ name: "me.png", mimeType: "image/png", buffer: PHOTO });
  await expect(iryna.getByText(en["photo.saved"])).toBeVisible();
  // In the profile and on the rail, a square photo.
  await expect(profile.locator("ortus-avatar img")).toBeVisible();
  const shown = await profile.locator("ortus-avatar img").evaluate((img: HTMLImageElement) => [img.naturalWidth, img.naturalHeight]);
  expect(shown).toEqual([256, 256]);
  await expect(iryna.locator("ortus-shell").getByRole("button", { name: en["account.menu"] }).locator("ortus-avatar img")).toBeVisible();
  // The owner sees it in the people list.
  await page.goto("/admin/users");
  await expect(page.getByRole("row").filter({ hasText: "Iryna Savchenko" }).last().locator("ortus-avatar img")).toBeVisible();

  await profile.getByRole("button", { name: en["photo.remove"] }).click();
  await expect(iryna.getByText(en["photo.removed"])).toBeVisible();
  await expect(profile.locator("ortus-avatar img")).toHaveCount(0);
});
