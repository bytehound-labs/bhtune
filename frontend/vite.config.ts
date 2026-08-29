import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    // Keep the dev UI reachable from other hosts on the local network.
    host: "0.0.0.0",
    allowedHosts: ["asus"],
    // Proxy API calls to a locally running `bhtune-server` in dev mode, so
    // `pnpm dev` gets hot-reload while still exercising the real HTTP API
    // instead of a mock. In production builds, bhtune-server instead embeds
    // and serves the built SPA directly from its own binary (via
    // `rust-embed` -- see `crates/bhtune-server/src/spa.rs`), so this proxy
    // is a dev-only concern.
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8787",
        changeOrigin: true,
      },
    },
  },
});
