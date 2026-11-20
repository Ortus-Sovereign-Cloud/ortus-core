// Creates the first owner through the real setup page, once, and keeps
// the signed-in session for every test (test-results/owner.json).
import { chromium, expect, type FullConfig } from "@playwright/test";

export const OWNER_STATE = "test-results/owner.json";
export const SETUP_TOKEN = "e2e-setup-token";

export default async function globalSetup(config: FullConfig): Promise<void> {
  const baseURL = config.projects[0]?.use.baseURL as string;
  const browser = await chromium.launch();
  const page = await browser.newPage({ baseURL });
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.goto(`/setup#${SETUP_TOKEN}`);
  await page.getByLabel("Your name").fill("Taras Bondar");
  await page.getByLabel("Work email").fill("taras@ministry.example");
  await page.getByRole("button", { name: "Create the owner" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.locator("ortus-shell")).toBeAttached();
  await page.context().storageState({ path: OWNER_STATE });
  await browser.close();
}
