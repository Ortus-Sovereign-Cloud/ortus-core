// Signing in with a password and an authenticator app, and with a
// recovery code, all set up from the security page by the owner.
import { createHmac } from "node:crypto";
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

function base32(text: string): Buffer {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of text) bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const bytes = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(parseInt(bits.slice(i, i + 8), 2));
  return Buffer.from(bytes);
}

function totp(secret: string, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigInt64BE(BigInt(step));
  const digest = createHmac("sha1", base32(secret)).update(counter).digest();
  const offset = digest[19]! & 0x0f;
  const number = digest.readUInt32BE(offset) & 0x7fffffff;
  return String(number % 1_000_000).padStart(6, "0");
}

async function signInPage(page: Page): Promise<void> {
  await page.goto("/sign-in");
  await expect(page.getByRole("heading", { name: en["signIn.title"] })).toBeVisible();
}

// The steps share one account, so they run in order.
test.describe.configure({ mode: "serial" });

const PASSWORD = "flood plan 2026 R-12";
let secret = "";
let codes: string[] = [];

test("the owner sets a password and an authenticator app", async ({ page }) => {
  await page.goto("/settings/security");
  await expect(page.getByRole("heading", { level: 1, name: en["security.title"] })).toBeVisible();
  await page.getByLabel(en["security.newPassword"]).fill(PASSWORD);
  await page.getByRole("button", { name: en["security.savePassword"] }).click();
  await expect(page.getByText(en["security.passwordSaved"])).toBeVisible();

  await page.getByRole("button", { name: en["security.setUpApp"] }).click();
  secret = (await page.locator("ortus-security code").textContent()) ?? "";
  expect(secret).toMatch(/^[A-Z2-7]{32}$/);
  await expect(page.locator("ortus-security .qr svg")).toBeVisible();
  await page.getByLabel(en["security.firstCode"]).fill(totp(secret, Math.floor(Date.now() / 30_000)));
  await page.getByRole("button", { name: en["security.addApp"] }).click();
  await expect(page.getByText(en["security.appAdded"])).toBeVisible();

  await page.getByRole("button", { name: en["security.makeKit"] }).click();
  await expect(page.locator("ortus-security .codes li")).toHaveCount(10);
  codes = await page.locator("ortus-security .codes li").allTextContents();
});

test.describe("someone signing in", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  test("uses the password, then the code from the app", async ({ page }) => {
    await signInPage(page);
    await page.getByLabel(en["signIn.email"]).fill("taras@ministry.example");
    await page.getByLabel(en["signIn.password"]).fill("not the password");
    await page.getByRole("button", { name: en["signIn.submit"], exact: true }).click();
    await expect(page.getByRole("alert")).toHaveText(en["signIn.wrong"]);

    await page.getByLabel(en["signIn.password"]).fill(PASSWORD);
    await page.getByRole("button", { name: en["signIn.submit"], exact: true }).click();
    await expect(page.getByText(en["signIn.codeLead"])).toBeVisible();
    // The enrolment code's step is used; the next one is accepted.
    await page.getByLabel(en["signIn.code"]).fill(totp(secret, Math.floor(Date.now() / 30_000) + 1));
    await page.getByRole("button", { name: en["signIn.confirm"] }).click();
    await expect(page).toHaveURL(/\/$/);
    await expect(page.locator("ortus-shell")).toBeAttached();
  });

  test("uses a recovery code, once", async ({ page }) => {
    await signInPage(page);
    await page.getByRole("button", { name: en["signIn.withRecovery"] }).click();
    await page.getByLabel(en["signIn.email"]).fill("taras@ministry.example");
    await page.getByLabel(en["signIn.recoveryCode"]).fill(codes[0]!);
    await page.getByRole("button", { name: en["signIn.submit"], exact: true }).click();
    await expect(page).toHaveURL(/\/$/);
    await page.context().clearCookies();
    await signInPage(page);
    await page.getByRole("button", { name: en["signIn.withRecovery"] }).click();
    await page.getByLabel(en["signIn.email"]).fill("taras@ministry.example");
    await page.getByLabel(en["signIn.recoveryCode"]).fill(codes[0]!);
    await page.getByRole("button", { name: en["signIn.submit"], exact: true }).click();
    await expect(page.getByRole("alert")).toHaveText(en["signIn.wrong"]);
  });
});

for (const locale of LOCALES) {
  const catalog = CATALOGS[locale.code as Code];
  test(`the security page is in ${locale.name}`, async ({ page }) => {
    await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
    await page.goto("/settings/security");
    await expect(page.getByRole("heading", { level: 1, name: catalog["security.title"] as string })).toBeVisible();
    await expect(page.getByRole("heading", { level: 2, name: catalog["security.kit"] as string })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", locale.dir);
  });
}

test("a passkey is added on the security page and signs in", async ({ page }) => {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  await page.goto("/settings/security");
  await page.getByLabel(en["security.passkeyName"]).fill("Work laptop");
  await page.getByRole("button", { name: en["security.addPasskey"] }).click();
  await expect(page.getByText(en["security.passkeyAdded"])).toBeVisible();
  await expect(page.locator("ortus-security .keys li")).toContainText("Work laptop");

  // Signed out, the same browser (and its authenticator) signs in again.
  await page.context().clearCookies();
  await page.goto("/sign-in");
  await page.getByRole("button", { name: en["signIn.withPasskey"] }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.locator("ortus-shell")).toBeAttached();
});

test.describe("an outside sign-in service", () => {
  test.use({ storageState: { cookies: [], origins: [] } });
  test.skip(!process.env.ORTUS_TEST_IDP, "needs a test identity provider (ORTUS_TEST_IDP)");

  test("signs the owner in from the sign-in page", async ({ page }) => {
    await page.goto("/sign-in");
    await page.getByRole("link", { name: en["signIn.withProvider"].replace("{name}", "National eID") }).click();
    // The test provider's own page.
    await page.locator('input[name="username"]').fill("taras-at-eid");
    await page.locator('textarea[name="claims"]').fill(JSON.stringify({ email: "taras@ministry.example", email_verified: true }));
    await page.locator('input[type="submit"]').click();
    await expect(page).toHaveURL(/localhost:\d+\/$/);
    await expect(page.locator("ortus-shell")).toBeAttached();
  });

  for (const locale of LOCALES) {
    const catalog = CATALOGS[locale.code as Code];
    test(`says why it did not work, in ${locale.name}`, async ({ page }) => {
      await page.addInitScript((code) => localStorage.setItem("ortus.locale", code), locale.code);
      await page.goto("/sign-in?external=no_account");
      await expect(page.getByRole("alert")).toHaveText(catalog["signIn.externalNoAccount"] as string);
    });
  }
});
