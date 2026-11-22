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

test("dropdowns show one chevron with room, mirrored right to left", async ({ page }) => {
  await page.goto("/settings");
  const select = page.locator("ortus-settings select#size");
  const style = await select.evaluate((el) => {
    const s = getComputedStyle(el);
    return { appearance: s.appearance, image: s.backgroundImage.startsWith("url("), end: s.paddingInlineEnd };
  });
  expect(style).toEqual({ appearance: "none", image: true, end: "34px" });
  // The three dropdowns, for a look.
  const first = (await page.locator("ortus-settings select#lang").boundingBox())!;
  const last = (await page.locator("ortus-settings select#motion").boundingBox())!;
  await page.screenshot({ path: "test-results/screens/settings-dropdowns.png", clip: { x: first.x - 260, y: first.y - 16, width: first.width + 280, height: last.y + last.height - first.y + 32 } });
});

test("a person signs in on their phone from the web with a one-time code", async ({ page, browser }) => {
  const { page: iryna, email } = await newcomer(page, browser);
  await iryna.goto("/");
  await iryna.getByRole("button", { name: en["account.menu"] }).click();
  const made = iryna.waitForResponse((r) => r.url().endsWith("/api/v1/me/phone-link") && r.request().method() === "POST");
  await iryna.getByRole("menu", { name: en["account.menu"] }).getByRole("menuitem", { name: en["account.phone"] }).click();
  const dialog = iryna.getByRole("dialog", { name: en["phone.title"] });
  await expect(dialog.getByRole("img", { name: en["phone.qr"] })).toBeVisible();
  await expect(dialog).toContainText(en["phone.once"]);
  const link = ((await (await made).json()) as { link: string }).link;
  const code = link.split("p=")[1]!;

  // On a computer, the link's page says to open it on the phone.
  const relay = await (await browser.newContext({ storageState: { cookies: [], origins: [] } })).newPage();
  await relay.goto(`/i#${link.split("#")[1]}`);
  await expect(relay.getByText(en["relay.phoneOnComputer"])).toBeVisible();

  // The app follows it: signed in as Iryna; the web says so.
  const phone = await relay.request.post("/api/v1/session/phone-link", { headers: { "X-Requested-With": "ortus", "User-Agent": "Ortus/1 CFNetwork/3826 Darwin/25.0.0" }, data: { token: code } });
  expect(phone.status()).toBe(200);
  expect(((await phone.json()) as { email: string }).email).toBe(email);
  await expect(dialog.getByRole("status")).toHaveText(en["phone.doneOn"].replace("{device}", "iPhone"));
  // Once only.
  expect((await relay.request.post("/api/v1/session/phone-link", { headers: { "X-Requested-With": "ortus" }, data: { token: code } })).status()).toBe(410);
});
