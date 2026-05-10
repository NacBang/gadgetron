import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    include: ["__tests__/**/*.{test,spec}.{ts,tsx}"],
    // CI-friendly: GitHub Actions runners are slower than local dev,
    // so the default 5s testTimeout flakes on async render+effect chains.
    testTimeout: 15000,
    hookTimeout: 15000,
    // Default `forks` pool spawns child processes that hit Node's heap
    // limit on the WorkbenchShell suite (jsdom + assistant-ui graph).
    // Threads share the parent heap and avoid the per-fork 1.7 GB ceiling.
    pool: "threads",
  },
  // Vitest 4 moved `poolOptions` to the top level (was `test.poolOptions`).
  poolOptions: {
    threads: {
      singleThread: true,
      // NODE_OPTIONS doesn't propagate into vitest worker threads, so
      // raise the heap explicitly via execArgv. The WorkbenchShell
      // suite (jsdom + assistant-ui) needs ~3 GB.
      execArgv: ["--max-old-space-size=8192"],
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./app"),
    },
  },
});
