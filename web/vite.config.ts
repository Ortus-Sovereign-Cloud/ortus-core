import { defineConfig } from "vitest/config";

// The build output is embedded in the Rust program (see ../src/web.rs).
// File names carry a content hash, so the server can let browsers keep
// them for a year.
export default defineConfig({
  build: { outDir: "dist", emptyOutDir: true, sourcemap: false, target: "es2022" },
  server: { proxy: { "/api": "http://127.0.0.1:7878" } },
  // Loading every catalog can take seconds on a busy CI runner.
  test: { environment: "node", include: ["tests/**/*.test.ts"], testTimeout: 30_000 },
});
