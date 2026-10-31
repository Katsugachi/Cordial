import { defineConfig } from "vitest/config";

// Tauri expects a fixed port in development, and the same bundle is opened
// directly in a browser when running the UI preview.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "0.0.0.0",
    allowedHosts: true,
    watch: { ignored: ["**/crates/**", "**/target/**"] },
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
  },
  test: {
    environment: "jsdom",
    include: ["tests/**/*.test.ts"],
  },
});
