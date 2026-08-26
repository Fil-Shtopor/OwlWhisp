import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// When developing against a physical device, Tauri sets TAURI_DEV_HOST.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],

  // Don't clear the terminal: it hides Rust build errors during `tauri dev`.
  clearScreen: false,

  // Expose Tauri's TAURI_ENV_* vars to the frontend (in addition to VITE_*).
  envPrefix: ["VITE_", "TAURI_ENV_"],

  server: {
    port: 5173,
    strictPort: true,
    host: host ?? false,
    hmr: host ? { protocol: "ws", host, port: 5174 } : undefined,
    watch: {
      // The Rust crate lives next door; don't restart vite on its changes.
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    outDir: "dist",
    target: "es2022",
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        overlay: fileURLToPath(new URL("./overlay.html", import.meta.url)),
      },
    },
  },
});
