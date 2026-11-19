import { defineConfig } from "@playwright/test";

// The tests drive the real program: the built client embedded in the
// Rust binary, on SQLite in a throwaway directory.
const port = Number(process.env.ORTUS_E2E_PORT ?? 7979);
const binary = process.env.ORTUS_E2E_BINARY ?? "../target/debug/ortus-core";

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${port}`, viewport: { width: 1440, height: 900 }, colorScheme: "dark" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: `rm -rf test-results/data && ${binary}`,
    env: { ORTUS_ROLES: "web", ORTUS_WEB_PORT: String(port), ORTUS_DATA_DIR: "test-results/data" },
    url: `http://127.0.0.1:${port}/api/v1/health`,
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
