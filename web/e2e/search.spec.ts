// Search and commands (board WebSearch): Ctrl K opens it from anywhere in
// the shell, in every language; the keyboard and Escape work; a search
// with nothing indexed says so and still offers its commands.
import { expect, test } from "@playwright/test";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import es from "../src/locales/es.js";
import fr from "../src/locales/fr.js";
import it from "../src/locales/it.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

for (const [code, texts] of [["en", en], ["pl", pl], ["uk", uk], ["de", de], ["fr", fr], ["es", es], ["it", it], ["ar", ar]] as const) {
  test(`Ctrl K opens the search in ${code}`, async ({ page }) => {
    await page.addInitScript((c) => localStorage.setItem("ortus.locale", c), code);
    await page.goto("/settings");
    await expect(page.locator("ortus-search")).toBeAttached();
    await page.keyboard.press("Control+k");
    const search = page.getByRole("dialog", { name: texts["search.title"] as string });
    await expect(search).toBeVisible();
    await expect(search.getByRole("searchbox")).toBeFocused();
    await expect(search.getByRole("group", { name: texts["search.filters"] as string }).getByRole("button", { name: texts["search.all"] as string })).toHaveAttribute("aria-pressed", "true");
  });
}

test("a search with nothing found offers its commands, and Escape closes it", async ({ page }) => {
  await page.goto("/settings");
  await expect(page.locator("ortus-search")).toBeAttached();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: en["search.title"] });
  await search.getByRole("searchbox").fill("gauge from:anna");
  await expect(search.getByRole("status").filter({ hasText: en["search.nothing"] })).toBeVisible();
  const commands = search.getByRole("list", { name: en["search.commands"] });
  await expect(commands).toContainText("Book a meeting with Anna");
  await expect(search.getByRole("group", { name: en["search.filters"] }).getByRole("button", { name: "from:anna" })).toHaveAttribute("aria-pressed", "true");
  await page.screenshot({ path: "test-results/search.png" });
  await page.keyboard.press("Escape");
  await expect(search).toBeHidden();
});
