#!/usr/bin/env bash
# Every check CI runs, in the same order. Run it before each push.
# Storage and file tests also need the services described in the README;
# without them those tests are skipped here (CI never skips them).
set -euo pipefail
cd "$(dirname "$0")/.."
(cd web && npm ci --silent && npx tsc --noEmit && npm test --silent && node tools/licences.mjs && node tools/licences.mjs --selftest && npm run build --silent)
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
ci/licence-gate-selftest.sh
(cd web && npx playwright test)
echo "all checks passed"
