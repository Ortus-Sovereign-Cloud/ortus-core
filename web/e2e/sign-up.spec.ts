// Open sign-up, as on the public demo: the email check through a real
// mail server, then the first passkey; and the terms in every language.
import { expect, test } from "@playwright/test";
import { LOCALES } from "../src/i18n.js";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import es from "../src/locales/es.js";
import fr from "../src/locales/fr.js";
import it from "../src/locales/it.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

const CATALOGS = { en, pl, uk, de, fr, es, it, ar } as const;
type Code = keyof typeof CATALOGS;
const MAILPIT = process.env.ORTUS_TEST_MAILPIT;

test.use({ storageState: { cookies: [], origins: [] } });
test.skip(!process.env.ORTUS_TEST_SMTP, "needs a test mail server (ORTUS_TEST_SMTP, ORTUS_TEST_MAILPIT)");

async function codeFor(email: string): Promise<string> {
  for (let i = 0; i < 50; i += 1) {
    const found = await (await fetch(`${MAILPIT}/api/v1/search?query=to:${encodeURIComponent(email)}`)).json();
    const id = found.messages?.[0]?.ID;
    if (id) {
      const message = await (await fetch(`${MAILPIT}/api/v1/message/${id}`)).json();
      const code = String(message.Text).match(/\b\d{6}\b/)?.[0];
      if (code) return code;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`no code arrived for ${email}`);
}

test("someone signs up with an email check and adds a passkey", async ({ page }) => {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  const email = `iryna-${Date.now()}@example.org`;
  await page.goto("/sign-in");
  await page.getByRole("link", { name: en["signIn.createAccount"] }).click();
  await expect(page).toHaveURL(/\/sign-up$/);
  await expect(page.getByText(en["signUp.lead"].replace("{days}", "30"))).toBeVisible();
  await page.getByLabel(en["signUp.name"]).fill("Iryna Savchenko");
  await page.getByLabel(en["signUp.email"]).fill(email);
  await page.getByRole("checkbox").check();
  await page.getByRole("button", { name: en["signUp.submit"] }).click();
  await expect(page.getByText(en["signUp.codeLead"].replace("{email}", email))).toBeVisible();
  await page.getByLabel(en["signUp.code"]).fill(await codeFor(email));
  await page.getByRole("button", { name: en["signUp.verify"] }).click();
  await expect(page).toHaveURL(/\/settings\/security\?welcome$/);
  await expect(page.getByText(en["security.welcome"])).toBeVisible();
  await page.getByRole("button", { name: en["security.addPasskey"] }).click();
  await expect(page.getByText(en["security.passkeyAdded"])).toBeVisible();

  await page.context().clearCookies();
  await page.goto("/sign-in");
  await page.getByRole("button", { name: en["signIn.withPasskey"] }).click();
  await expect(page).toHaveURL(/\/$/);
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the terms and the sign-up page are in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/terms");
    await expect(page.getByRole("heading", { level: 1, name: catalog["terms.title"] as string })).toBeVisible();
    await expect(page.getByText((catalog["terms.deletion"] as string).replace("{days}", "30"))).toBeVisible();
    await page.goto("/sign-up");
    await expect(page.getByRole("heading", { level: 1, name: catalog["signUp.title"] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}
