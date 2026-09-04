import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    // When the desktop Tauri shell is unavailable (headless / remote), the
    // frontend talks to `skills-manager-web` over HTTP. The Tauri CLI proxy
    // has its own /api prefix — keep ours distinct so they don't collide.
    proxy: {
      "/skillsmanager": {
        target: "http://127.0.0.1:8766",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/skillsmanager/, "/api"),
      },
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    proxy: {
      "/skillsmanager": {
        target: "http://127.0.0.1:8766",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/skillsmanager/, "/api"),
      },
    },
  },
});
