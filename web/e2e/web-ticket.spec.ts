// The phone's way into a web page: a one-time ticket in the address's
// fragment; the page asks before signing the browser in, then goes on to
// the path; the same link does not work twice.
import { expect, test } from "@playwright/test";
import en from "../src/locales/en.js";

test("a ticket from the app signs a browser in once, on the person's yes", async ({ page, browser }) => {
  const made = await page.request.post("/api/v1/me/web-ticket", { headers: { "X-Requested-With": "ortus" }, data: { next: "/settings" } });
  expect(made.status(), await made.text()).toBe(200);
  const url = new URL(((await made.json()) as { url: string }).url);
  const fresh = await (await browser.newContext({ storageState: { cookies: [], origins: [] } })).newPage();
  await fresh.goto(url.pathname + url.hash);
  // The ticket leaves the address at once.
  await expect(fresh).toHaveURL(/\/session\/web$/);
  await expect(fresh.getByText(/Continue as .+ \(.+@.+\)\?/)).toBeVisible();
  await fresh.getByRole("button", { name: en["webTicket.continue"] }).click();
  await expect(fresh).toHaveURL(/\/settings$/);
  await expect(fresh.getByRole("navigation", { name: en["shell.apps"] })).toBeVisible();

  // Once only.
  const again = await (await browser.newContext({ storageState: { cookies: [], origins: [] } })).newPage();
  await again.goto(url.pathname + url.hash);
  await expect(again.getByRole("alert")).toHaveText(en["webTicket.expired"]);
  await expect(again.getByRole("button", { name: en["webTicket.continue"] })).toHaveCount(0);
  await fresh.context().close();
  await again.context().close();
});
