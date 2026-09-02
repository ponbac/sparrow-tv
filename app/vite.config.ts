import path from "path";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// https://vitejs.dev/config/
export default defineConfig({
  base: process.env.TAURI_ENV_PLATFORM === undefined ? "/app/" : "/",
  plugins: [react()],
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("commonjsHelpers")) {
            return "commonjs-runtime";
          }
          return id.includes("/node_modules/mpegts.js/")
            ? "mpegts"
            : undefined;
        },
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
  resolve: {
    alias: {
      // The published UMD bundle bakes in legacy polyfill assignments that
      // conflict with Tauri's `freezePrototype` hardening. Bundle the audited,
      // repository-patched source entry so modern built-ins are left intact.
      "mpegts.js": path.resolve(
        __dirname,
        "./node_modules/mpegts.js/src/mpegts.js",
      ),
    },
  },
});
