import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Backend (Rust server or the mock) that /api and /ws are proxied to in dev.
const backend = process.env.SM_BACKEND ?? "http://127.0.0.1:7400";

export default defineConfig({
  plugins: [react()],
  base: "/",
  build: {
    outDir: "dist",
    assetsDir: "assets",
    sourcemap: false,
    target: "es2020",
    // Never inline-reference anything external; everything is bundled.
    modulePreload: { polyfill: false },
  },
  server: {
    port: 5173,
    proxy: {
      // changeOrigin: the backend sees its own host name, like a request from the local machine
      "/api": { target: backend, changeOrigin: true },
      "/ws": { target: backend, ws: true, changeOrigin: true },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
