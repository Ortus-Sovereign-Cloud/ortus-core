import { expect, test, type Page } from "@playwright/test";
import { LOCALES } from "../src/i18n.js";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import fr from "../src/locales/fr.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

const CATALOGS = { en, pl, uk, de, fr, ar } as const;
type Code = keyof typeof CATALOGS;

/** The apps of version 1, as the server will list them once they exist. */
const V1_APPS = [
  { id: "mail", group: "communicate", path: "/mail" },
  { id: "chat", group: "communicate", path: "/chat" },
  { id: "calls", group: "communicate", path: "/calls" },
  { id: "calendar", group: "organise", path: "/calendar" },
  { id: "tasks", group: "organise", path: "/tasks" },
  { id: "people", group: "organise", path: "/people" },
  { id: "drive", group: "create", path: "/drive" },
  { id: "docs", group: "create", path: "/docs" },
  { id: "approvals", group: "create", path: "/approvals" },
  { id: "admin", group: "organisation", path: "/admin" },
  { id: "settings", group: "organisation", path: "/settings" },
];

async function withApps(page: Page): Promise<void> {
  await page.route("**/api/v1/apps", (route) => route.fulfill({ json: { apps: V1_APPS } }));
}

/** Fails the test on anything the browser complains about, such as a blocked script or style. */
function watchForErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(String(error)));
  return errors;
}

async function chooseLanguage(page: Page, code: Code): Promise<void> {
  await page.locator("ortus-shell nav .language > button").click();
  await page.locator(`ortus-shell [role="menuitemradio"][lang="${code}"]`).click();
  await expect(page.locator("html")).toHaveAttribute("lang", code);
}

test("a fresh installation says no apps are installed, with nothing blocked", async ({ page }) => {
  const errors = watchForErrors(page);
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1, name: en["switcher.title"] })).toBeVisible();
  await expect(page.getByText(en["switcher.empty"])).toBeVisible();
  await expect(page.getByText(en["switcher.emptyHint"])).toBeVisible();
  await expect(page.locator("ortus-app-switcher a.app")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("the rail and the switcher show the installed apps", async ({ page }) => {
  const errors = watchForErrors(page);
  await withApps(page);
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: en["shell.apps"] });
  // Home, six apps, Admin (the owner is an administrator), All apps and My settings.
  await expect(nav.getByRole("link")).toHaveCount(10);
  await expect(nav.locator(".label")).toHaveText(["Mail", "Chat", "Calls", "Calendar", "Tasks", "People", "Admin", "All apps"]);
  await expect(nav.getByRole("link", { name: en["shell.home"] })).toHaveAttribute("href", "/");
  await expect(nav.getByRole("link", { name: "All apps", exact: true })).toHaveAttribute("aria-current", "page");

  await expect(page.getByRole("heading", { level: 2 })).toHaveText(["Communicate", "Organise", "Create and record", "Organisation"]);
  const tiles = page.locator("ortus-app-switcher a.app");
  await expect(tiles).toHaveText(["Mail", "Chat", "Calls", "Calendar", "Tasks", "People", "Drive", "Docs", "Approvals", "Admin", "Settings"]);
  await expect(tiles.first()).toHaveAttribute("href", "/mail");
  await expect(page.locator("ortus-app-switcher a.app svg")).toHaveCount(11);
  await expect(page.getByText("11 apps")).toBeVisible();

  const find = page.getByRole("searchbox", { name: en["switcher.find"] });
  await find.fill("ca");
  await expect(tiles).toHaveText(["Calls", "Calendar"]);
  await expect(page.getByText("2 apps")).toBeVisible();
  await find.fill("zzz");
  await expect(page.getByText("No app matches “zzz”.")).toBeVisible();
  expect(errors).toEqual([]);
});

for (const locale of LOCALES) {
  const code = locale.code as Code;
  const catalog = CATALOGS[code];
  test(`the whole screen is in ${locale.name} (${code}), ${locale.dir}`, async ({ page }) => {
    const errors = watchForErrors(page);
    await withApps(page);
    await page.goto("/");
    await chooseLanguage(page, code);
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(catalog["switcher.title"] as string);
    await expect(page.getByRole("heading", { level: 2 })).toHaveText([catalog["group.communicate"], catalog["group.organise"], catalog["group.create"], catalog["group.organisation"]] as string[]);
    await expect(page.locator("ortus-app-switcher a.app")).toHaveText(V1_APPS.map((app) => catalog[`app.${app.id}` as keyof typeof en] as string));
    await expect(page.getByRole("searchbox")).toHaveAttribute("placeholder", catalog["switcher.find"] as string);
    await expect(page.getByRole("navigation")).toHaveAttribute("aria-label", catalog["shell.apps"] as string);

    // No English is left on the screen (English itself excepted, and words
    // that are the same in both languages, such as "Chat" in German).
    if (code !== "en") {
      const shown = await page.locator("ortus-shell").innerText();
      const same = new Set(Object.keys(en).filter((key) => JSON.stringify(en[key as keyof typeof en]) === JSON.stringify(catalog[key as keyof typeof en])));
      for (const [key, message] of Object.entries(en)) {
        if (typeof message !== "string" || same.has(key) || !key.startsWith("app.") && !key.startsWith("group.") && key !== "switcher.title") continue;
        const stillEnglish = shown.split("\n").some((line) => line.trim() === message);
        expect(stillEnglish, `"${message}" (${key}) is still in English`).toBe(false);
      }
    }

    // Nothing in the rail is cut off, however long the translation.
    for (const label of await page.locator("ortus-shell nav .label").all()) {
      expect(await label.evaluate((el) => el.scrollHeight <= el.clientHeight + 1 && el.scrollWidth <= el.clientWidth + 1), await label.innerText()).toBe(true);
    }

    // The choice is remembered.
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("lang", code);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(catalog["switcher.title"] as string);
    await page.screenshot({ path: `test-results/screens/shell-${code}.png` });
    expect(errors).toEqual([]);
  });
}

test("a right-to-left language mirrors the layout", async ({ page }) => {
  await withApps(page);
  await page.goto("/");
  const width = page.viewportSize()!.width;
  const box = async (selector: string) => (await page.locator(selector).boundingBox())!;

  // Left to right: rail on the left, panel beside it, title before the search field.
  let rail = await box("ortus-shell nav");
  let panel = await box("ortus-app-switcher section");
  let title = await box("ortus-app-switcher h1");
  let find = await box("ortus-app-switcher input");
  let firstTile = await box("ortus-app-switcher a.app >> nth=0");
  let secondTile = await box("ortus-app-switcher a.app >> nth=1");
  expect(rail.x).toBe(0);
  expect(panel.x).toBeCloseTo(rail.width + 12, 0);
  expect(title.x).toBeLessThan(find.x);
  expect(firstTile.x).toBeLessThan(secondTile.x);

  await chooseLanguage(page, "ar");
  rail = await box("ortus-shell nav");
  panel = await box("ortus-app-switcher section");
  title = await box("ortus-app-switcher h1");
  find = await box("ortus-app-switcher input");
  firstTile = await box("ortus-app-switcher a.app >> nth=0");
  secondTile = await box("ortus-app-switcher a.app >> nth=1");
  // Right to left: the exact mirror image.
  expect(rail.x + rail.width).toBe(width);
  expect(panel.x + panel.width).toBeCloseTo(width - rail.width - 12, 0);
  expect(title.x).toBeGreaterThan(find.x);
  expect(firstTile.x).toBeGreaterThan(secondTile.x);
  await expect(page.locator("ortus-app-switcher h1")).toHaveCSS("direction", "rtl");

  // The language menu opens beside the rail, on its inner side.
  await page.locator("ortus-shell nav .language > button").click();
  const menu = await box("ortus-shell [popover]");
  expect(menu.x + menu.width).toBeCloseTo(width - rail.width - 8, 0);
  // Each language is written in its own script and direction.
  await expect(page.locator('ortus-shell [role="menuitemradio"]')).toHaveText(LOCALES.map((locale) => locale.name));
  await expect(page.locator('ortus-shell [role="menuitemradio"][lang="en"]')).toHaveAttribute("dir", "ltr");

  // Back to a left-to-right language: the layout returns.
  await page.locator('ortus-shell [role="menuitemradio"][lang="pl"]').click();
  await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
  expect((await box("ortus-shell nav")).x).toBe(0);
});

test.describe("first visit", () => {
  for (const [browser, expected] of [["uk-UA", "uk"], ["ar-EG", "ar"], ["de-AT", "de"], ["fr-CA", "fr"], ["pl", "pl"], ["ja-JP", "en"]] as const) {
    test.describe(`with a browser set to ${browser}`, () => {
      test.use({ locale: browser });
      test(`starts in ${expected}`, async ({ page }) => {
        await page.goto("/");
        await expect(page.locator("html")).toHaveAttribute("lang", expected);
        await expect(page.getByRole("heading", { level: 1 })).toHaveText(CATALOGS[expected]["switcher.title"] as string);
      });
    });
  }
});

test("a failed load says so and can be retried", async ({ page }) => {
  let fail = true;
  await page.route("**/api/v1/apps", (route) => (fail ? route.fulfill({ status: 500, json: { error: { code: "internal", message: "x", request_id: "r" } } }) : route.fulfill({ json: { apps: V1_APPS } })));
  await page.goto("/");
  await expect(page.getByRole("alert")).toHaveText(en["switcher.loadFailed"]);
  fail = false;
  await page.getByRole("button", { name: en["switcher.retry"] }).click();
  await expect(page.locator("ortus-app-switcher a.app")).toHaveCount(11);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("the keyboard reaches everything, starting with a skip link", async ({ page }) => {
  await withApps(page);
  await page.goto("/");
  await expect(page.locator("ortus-app-switcher a.app")).toHaveCount(11);
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: en["shell.skip"] });
  await expect(skip).toBeFocused();
  await expect(skip).toBeInViewport();
  await page.keyboard.press("Tab");
  await expect(page.getByRole("link", { name: en["shell.home"] })).toBeFocused();
});

test("it fits a phone-width window without sideways scrolling", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await withApps(page);
  await page.goto("/");
  await expect(page.locator("ortus-app-switcher a.app")).toHaveCount(11);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: "test-results/screens/shell-en-phone.png" });
});

test.describe("an address that belongs to no installed app", () => {
  for (const locale of LOCALES) {
    test(`says the app is not installed, in ${locale.name} (${locale.code})`, async ({ page }) => {
      const errors = watchForErrors(page);
      await page.goto("/");
      await chooseLanguage(page, locale.code as Code);
      await page.goto("/drive/some/file");
      const catalog = CATALOGS[locale.code as Code];
      await expect(page.getByRole("heading", { level: 1, name: catalog["notInstalled.title"] as string })).toBeVisible();
      await expect(page.getByText((catalog["notInstalled.body"] as string).replace("{path}", "/drive/some/file"))).toBeVisible();
      await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
      await page.getByRole("link", { name: catalog["notInstalled.back"] as string }).click();
      await expect(page).toHaveURL(/\/$/);
      await expect(page.getByRole("heading", { level: 1, name: catalog["switcher.title"] as string })).toBeVisible();
      expect(errors).toEqual([]);
    });
  }
});

// An app's client: the shell imports /assets/apps/<id>/app.js when the
// app's path opens; the app builds on the kit (/assets/kit.js) and its
// texts follow the page's language.
const TASKS_APP = `
import { LitElement, html, LocalizeController, appText, registerAppMessages, controls } from "/assets/kit.js";
const texts = { en: { title: "My tasks" }, pl: { title: "Moje zadania" }, uk: { title: "Мої завдання" }, de: { title: "Meine Aufgaben" }, fr: { title: "Mes tâches" }, ar: { title: "مهامي" } };
await registerAppMessages("tasks", texts.en, async (code) => texts[code] ?? {});
class TasksApp extends LitElement {
  static styles = [controls];
  localize = new LocalizeController(this);
  count = 0;
  render() { return html\`<h1>\${appText(this.localize, "tasks", "title")}</h1><button class="button" @click=\${() => { this.count += 1; this.requestUpdate(); }}>\${this.count}</button>\`; }
}
customElements.define("ortus-tasks-app", TasksApp);
`;

test("an app's own client loads at its path, shares the kit and follows the language", async ({ page }) => {
  const errors = watchForErrors(page);
  await withApps(page);
  await page.route("**/assets/apps/tasks/app.js", (route) => route.fulfill({ body: TASKS_APP, contentType: "text/javascript" }));
  // The stand-in app is not installed on the test server, so its place is not kept.
  await page.route("**/api/v1/me/place", (route) => route.fulfill({ json: {} }));
  await page.goto("/tasks");
  await expect(page.getByRole("heading", { level: 1, name: "My tasks" })).toBeVisible();
  // The element is kept: its state survives the shell rendering again.
  await page.locator("ortus-tasks-app").getByRole("button").click();
  await chooseLanguage(page, "pl");
  await expect(page.getByRole("heading", { level: 1, name: "Moje zadania" })).toBeVisible();
  await expect(page.locator("ortus-tasks-app").getByRole("button")).toHaveText("1");
  await chooseLanguage(page, "ar");
  await expect(page.getByRole("heading", { level: 1, name: "مهامي" })).toBeVisible();
  expect(errors).toEqual([]);
});

test("an app without a client of its own says it is not installed", async ({ page }) => {
  await withApps(page);
  await page.goto("/calls");
  await expect(page.getByRole("heading", { name: en["notInstalled.title"] })).toBeVisible();
});
