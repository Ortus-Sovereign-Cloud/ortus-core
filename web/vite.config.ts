import { defineConfig } from "vitest/config";

// The build output is embedded in the Rust program (see ../src/web.rs).
// File names carry a content hash, so the server can let browsers keep
// them for a year.
export default defineConfig({
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    target: "es2022",
    rollupOptions: {
      // The kit (src/kit.ts) is a second entry at a fixed name, so apps can
      // import it from /assets/kit.js; it shares its chunks (Lit, the
      // translations) with the page, so there is one of each.
      input: { main: "index.html", kit: "src/kit.ts" },
      preserveEntrySignatures: "exports-only",
      output: { entryFileNames: (chunk) => (chunk.name === "kit" ? "assets/kit.js" : "assets/[name]-[hash].js") },
    },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7878" } },
  // Loading every catalog can take seconds on a busy CI runner.
  test: { environment: "node", include: ["tests/**/*.test.ts"], testTimeout: 30_000 },
});
