import { defineConfig } from 'vite';

// hc-spin sets UI_PORT and opens one Electron window per agent pointed at this dev
// server, injecting each window's own conductor connection info — nothing to wire
// up here for that. See ../README.md.
export default defineConfig({
  server: {
    port: Number(process.env.UI_PORT) || 5173,
    strictPort: true,
  },
});
