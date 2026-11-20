import { defineConfig } from "@playwright/test";

// The tests drive the real program: the built client embedded in the
// Rust binary, on SQLite in a throwaway directory.
const port = Number(process.env.ORTUS_E2E_PORT ?? 7979);
const binary = process.env.ORTUS_E2E_BINARY ?? "../target/debug/ortus-core";
// With a test mail server (ORTUS_TEST_SMTP), the server opens sign-up as
// the public demo does; the sign-up tests read the codes from Mailpit.
const smtp = process.env.ORTUS_TEST_SMTP?.split(":");
const signup: Record<string, string> = smtp
  ? { ORTUS_SIGNUP_OPEN: "true", ORTUS_MAIL_HOST: smtp[0]!, ORTUS_MAIL_PORT: smtp[1]!, ORTUS_MAIL_SECURITY: "none", ORTUS_MAIL_FROM: "Ortus <noreply@ortus.test>" }
  : {};

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"]],
  globalSetup: "./e2e/global-setup.ts",
  // localhost, not 127.0.0.1: passkeys need a host name.
  use: { baseURL: `http://localhost:${port}`, viewport: { width: 1440, height: 900 }, colorScheme: "dark", storageState: "test-results/owner.json" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: `rm -rf test-results/data && ${binary}`,
    env: { ORTUS_ROLES: "web,worker", ORTUS_WEB_PORT: String(port), ORTUS_DATA_DIR: "test-results/data", ORTUS_SETUP_TOKEN: "e2e-setup-token", ORTUS_PUBLIC_URL: `http://localhost:${port}`, ORTUS_SITE_WEBSITE: "true", ...signup },
    url: `http://127.0.0.1:${port}/api/v1/health`,
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
