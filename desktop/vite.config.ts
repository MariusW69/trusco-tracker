import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    watch: { ignored: ["**/src-tauri/**", "**/core/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
