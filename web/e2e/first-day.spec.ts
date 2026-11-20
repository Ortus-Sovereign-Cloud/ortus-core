// A newcomer's first day: the invitation link, the welcome page and
// personal settings. Each test invites its own person (by mail, through
// the test mail server), so the shared owner's settings stay untouched.
import { expect, test, type Browser, type Page } from "@playwright/test";
import { LOCALES } from "../src/i18n.js";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import fr from "../src/locales/fr.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

const CATALOGS = { en, pl, uk, de, fr, ar } as const;
type Code = keyof typeof CATALOGS;
const MAILPIT = process.env.ORTUS_TEST_MAILPIT;

async function linkFor(email: string): Promise<string> {
  for (let i = 0; i < 50; i += 1) {
    const found = await (await fetch(`${MAILPIT}/api/v1/search?query=to:${encodeURIComponent(email)}`)).json();
    const id = found.messages?.[0]?.ID;
    if (id) {
      const message = await (await fetch(`${MAILPIT}/api/v1/message/${id}`)).json();
      const link = String(message.Text).match(/https?:\/\/\S+\/welcome\/accept#[A-Za-z0-9_-]+/)?.[0];
      if (link) return link;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`no invitation arrived for ${email}`);
}

/** The owner invites a newcomer; the newcomer follows the link in a browser of their own. */
async function newcomer(owner: Page, browser: Browser, name: string, locale?: string): Promise<Page> {
  const email = `${name.split(" ")[0]!.toLowerCase()}-${Date.now()}-${Math.floor(Math.random() * 1e6)}@ministry.example`;
  const invited = await owner.request.post("/api/v1/admin/users", {
    headers: { "X-Requested-With": "ortus" },
    data: { email, name, groups: ["Logistics"] },
  });
  expect(invited.status()).toBe(200);
  const link = new URL(await linkFor(email));
  const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  const page = await context.newPage();
  if (locale) await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale);
  await page.goto(link.pathname + link.hash);
  await expect(page).toHaveURL(/\/welcome$/);
  return page;
}

test.describe("an invitation link", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  test("that was used or never existed says so", async ({ page }) => {
    await page.goto("/welcome/accept#not-a-real-invitation");
    await expect(page.getByRole("alert")).toHaveText(en["accept.expired"]);
    await expect(page).toHaveURL(/\/welcome\/accept$/); // the token left the address bar
  });
});

test.describe("with a test mail server", () => {
  test.skip(!process.env.ORTUS_TEST_SMTP, "needs a test mail server (ORTUS_TEST_SMTP, ORTUS_TEST_MAILPIT)");

  test("a newcomer follows the invitation and works through the first day", async ({ page, browser }) => {
    const iryna = await newcomer(page, browser, "Iryna Savchenko");
    await expect(iryna.getByRole("heading", { level: 1, name: en["welcome.title"].replace("{name}", "Iryna") })).toBeVisible();
    await expect(iryna.getByText(en["welcome.team.detailGroups"].replace("{groups}", "Logistics"))).toBeVisible();
    await expect(iryna.getByText(en["welcome.help"])).toBeVisible();
    await iryna.getByRole("button", { name: en["welcome.team.action"] }).click();
    await expect(iryna.getByRole("region", { name: en["welcome.team.title"] }).getByText(en["overview.done"], { exact: true })).toBeVisible();
    const terms = iryna.context().waitForEvent("page");
    await iryna.getByRole("link", { name: en["welcome.policies.action"] }).click();
    await (await terms).close();
    await expect(iryna.getByText(en["overview.progress"].replace("{done}", "2").replace("{total}", "3"))).toBeVisible();
    await expect(iryna.getByRole("link", { name: en["welcome.secure.action"] })).toHaveAttribute("href", "/settings/security");
  });

  test("a newcomer changes text size, motion, language and name", async ({ page, browser }) => {
    const marek = await newcomer(page, browser, "Marek Nowak");
    await marek.goto("/settings");
    await marek.getByLabel(en["settings.textSize"]).selectOption("125");
    await expect(marek.locator("html")).toHaveAttribute("data-text-size", "125");
    await marek.getByLabel(en["settings.reduceMotion"]).selectOption("reduce");
    await expect(marek.locator("html")).toHaveAttribute("data-motion", "reduce");
    await marek.reload();
    await expect(marek.locator("html")).toHaveAttribute("data-text-size", "125");

    await marek.getByLabel(en["people.name"], { exact: true }).fill("Marek A. Nowak");
    await marek.getByRole("button", { name: en["settings.saveName"] }).click();
    await expect(marek.getByRole("status")).toHaveText(en["settings.saved"]);

    await expect(marek.getByText(en["settings.thisDevice"])).toBeVisible();
    await marek.locator("ortus-settings").getByLabel(en["settings.language"], { exact: true }).selectOption("pl");
    await expect(marek.locator("html")).toHaveAttribute("lang", "pl");
    await expect(marek.getByRole("heading", { name: pl["settings.title"] as string })).toBeVisible();
    await marek.reload();
    await expect(marek.locator("html")).toHaveAttribute("lang", "pl");

    await marek.getByRole("button", { name: pl["settings.signOut"] as string }).click();
    await expect(marek).toHaveURL(/\/sign-in$/);
  });

  for (const locale of LOCALES) {
    const catalog = CATALOGS[locale.code as Code];
    test(`the first day and settings are in ${locale.name}`, async ({ page, browser }) => {
      const person = await newcomer(page, browser, "Sam Kim", locale.code);
      await expect(person.getByRole("heading", { name: catalog["welcome.secure.title"] as string })).toBeVisible();
      await expect(person.locator("html")).toHaveAttribute("dir", locale.dir);
      await person.goto("/settings");
      await expect(person.getByRole("heading", { name: catalog["settings.languageAccessibility"] as string })).toBeVisible();
    });
  }

  test("screenshots of the first day and settings for comparing with the boards", async ({ page, browser }) => {
    const iryna = await newcomer(page, browser, "Iryna Savchenko");
    await expect(iryna.getByRole("heading", { name: en["welcome.secure.title"] })).toBeVisible();
    await iryna.screenshot({ path: "test-results/welcome.png" });
    await iryna.goto("/settings");
    await expect(iryna.getByText(en["settings.thisDevice"])).toBeVisible();
    await iryna.screenshot({ path: "test-results/settings.png" });
  });
});
