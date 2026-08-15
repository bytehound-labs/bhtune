import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    // Proxy API calls to a locally running `bhtune-server` in dev mode, so
    // `pnpm dev` gets hot-reload while still exercising the real HTTP API
    // instead of a mock. `server-embed-spa` replaces this with the SPA being
    // served directly by bhtune-server itself in production builds.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
      },
    },
  },
});
