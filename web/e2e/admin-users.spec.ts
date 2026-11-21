// Admin › Users & groups, as the owner.
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

test("the owner invites someone, imports a list and suspends a person", async ({ page }) => {
  await page.goto("/admin/users");
  await expect(page.getByRole("heading", { level: 1, name: en["admin.users"] })).toBeVisible();
  await expect(page.getByRole("row").filter({ hasText: "taras@ministry.example" })).toBeVisible();

  await page.getByRole("button", { name: en["people.invite"] }).click();
  await page.getByLabel(en["people.name"], { exact: true }).fill("Iryna Savchenko");
  await page.getByLabel(en["people.email"], { exact: true }).fill("iryna@ministry.example");
  await page.getByLabel(en["people.groupsField"]).fill("Procurement");
  await page.getByRole("button", { name: en["people.send"] }).click();
  const sent = page.getByRole("dialog", { name: en["people.invitationFor"].replace("{name}", "Iryna Savchenko") });
  // With a test mail server the letter goes; without one, the link is the way.
  const said = process.env.ORTUS_TEST_SMTP ? en["people.sentTo"].replace("{address}", "iryna@ministry.example") : en["people.notSent.no_mail"];
  await expect(sent.getByRole("status")).toHaveText(said);
  await expect(sent.getByLabel(en["people.link"], { exact: true })).toHaveValue(/\/welcome\/accept#/);
  await sent.getByRole("button", { name: en["people.done"] }).click();
  const iryna = page.getByRole("row").filter({ hasText: "iryna@ministry.example" });
  await expect(iryna).toContainText(en["people.state.invited"]);

  await page.locator("#csv").setInputFiles({
    name: "people.csv",
    mimeType: "text/csv",
    buffer: Buffer.from("email,name,role,groups\njonas@ministry.example,Jonas Berg,member,Procurement\nbroken line\n"),
  });
  await expect(page.getByRole("alert")).toHaveText(en["people.imported"].replace("{added}", "1").replace("{failed}", "1"));
  await expect(page.getByRole("row").filter({ hasText: "jonas@ministry.example" })).toBeVisible();

  await page.getByLabel(en["people.filter"]).fill("procurement");
  await expect(page.getByRole("row")).toHaveCount(3); // the header and two people
  await page.getByLabel(en["people.filter"]).fill("");
  await expect(page.getByRole("row").filter({ hasText: "taras@ministry.example" })).toBeVisible();

  await page.getByRole("row").filter({ hasText: "jonas@ministry.example" }).click();
  await expect(page.getByRole("complementary", { name: "Jonas Berg" })).toBeVisible();
  await page.getByRole("button", { name: en["people.suspend"], exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(en["people.suspended"]);
  await expect(page.getByRole("row").filter({ hasText: "jonas@ministry.example" })).toContainText(en["people.state.suspended"]);
});

test("the rail has the admin console for administrators", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("navigation").getByRole("link", { name: en["app.admin"] }).click();
  await expect(page).toHaveURL(/\/admin$/);
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the users page is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/admin/users");
    await expect(page.getByRole("heading", { level: 1, name: catalog["admin.users"] as string })).toBeVisible();
    await expect(page.getByRole("button", { name: catalog["people.invite"] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}

test("screenshot for comparing with the board", async ({ page }) => {
  await page.goto("/admin/users");
  await page.getByRole("row").filter({ hasText: "iryna@ministry.example" }).click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: "test-results/admin-users.png" });
});

test("the owner connects the directory and brings people in", async ({ page }) => {
  test.skip(!process.env.ORTUS_TEST_LDAP, "needs a test directory (ORTUS_TEST_LDAP)");
  await page.goto("/admin/users");
  await page.getByRole("button", { name: en["people.connectDirectory"] }).click();
  await page.getByLabel(en["people.directoryUrl"]).fill(process.env.ORTUS_TEST_LDAP!);
  await page.getByLabel(en["people.directoryBind"]).fill("cn=admin,dc=ministry,dc=example");
  await page.getByLabel(en["people.directoryPassword"]).fill("adminpass");
  await page.getByLabel(en["people.directoryBase"]).fill("dc=ministry,dc=example");
  await page.getByRole("button", { name: en["people.connect"], exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(en["people.directoryConnected"]);
  await page.getByRole("button", { name: en["people.syncDirectory"] }).click();
  await expect(page.getByRole("status")).toHaveText(en["people.directorySynced"].replace("{created}", "2").replace("{updated}", "0").replace("{groups}", "1"));
  await expect(page.getByRole("row").filter({ hasText: "olena@ministry.example" })).toBeVisible();
});
