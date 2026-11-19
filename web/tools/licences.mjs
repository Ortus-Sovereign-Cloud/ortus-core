// The licence gate for the web client: every package that ships in the
// built client must carry a permissive licence. Fonts are the one
// exception: they are data, not code, and carry the SIL Open Font
// License. Do not add a licence here without the owner's agreement.
//
// Usage: node tools/licences.mjs            checks the installed packages
//        node tools/licences.mjs --selftest proves a copyleft package fails

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const ALLOWED = new Set(["MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "CC0-1.0", "Unlicense", "BlueOak-1.0.0"]);
const FONT_LICENCES = new Set(["OFL-1.1"]);
const isFont = (name) => name.startsWith("@fontsource/") || name.startsWith("@fontsource-variable/");

/** Whether an SPDX expression is acceptable: every AND part, any OR part. */
export function allowed(name, expression) {
  if (!expression || typeof expression !== "string") return false;
  const one = (id) => ALLOWED.has(id) || (isFont(name) && FONT_LICENCES.has(id));
  const clean = expression.replace(/[()]/g, " ").trim();
  return clean.split(/\s+OR\s+/).some((choice) => choice.split(/\s+AND\s+/).every((id) => one(id.trim())));
}

/** Every package that ships: the production dependencies, transitively. */
function shipped(root) {
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  return Object.entries(lock.packages)
    .filter(([path, entry]) => path.startsWith("node_modules/") && !entry.dev)
    .map(([path]) => path);
}

function check(root) {
  const problems = [];
  const paths = shipped(root);
  for (const path of paths) {
    const file = join(root, path, "package.json");
    if (!existsSync(file)) continue; // an optional package for another platform
    const pkg = JSON.parse(readFileSync(file, "utf8"));
    if (!allowed(pkg.name, pkg.license)) problems.push(`${pkg.name}@${pkg.version}: ${JSON.stringify(pkg.license ?? null)}`);
  }
  return { count: paths.length, problems };
}

if (process.argv.includes("--selftest")) {
  const cases = [
    ["lit", "BSD-3-Clause", true],
    ["x", "MIT OR GPL-3.0", true],
    ["x", "(MIT AND Apache-2.0)", true],
    ["x", "GPL-3.0", false],
    ["x", "AGPL-3.0-only", false],
    ["x", "MPL-2.0", false],
    ["x", "LGPL-2.1", false],
    ["x", "MIT AND GPL-2.0", false],
    ["x", undefined, false],
    ["x", "OFL-1.1", false],
    ["@fontsource-variable/geist", "OFL-1.1", true],
    ["@fontsource-variable/geist", "GPL-3.0", false],
  ];
  const wrong = cases.filter(([name, licence, expected]) => allowed(name, licence) !== expected);
  if (wrong.length) {
    console.error("licence gate self-test FAILED:", wrong);
    process.exit(1);
  }
  console.log(`licence gate works: ${cases.length} cases, copyleft refused`);
} else {
  const { count, problems } = check(process.cwd());
  if (problems.length) {
    console.error(`licences not allowed in the web client:\n  ${problems.join("\n  ")}`);
    process.exit(1);
  }
  console.log(`web licences ok: ${count} shipped packages`);
}
