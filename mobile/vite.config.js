// Tauri n'a pas de serveur Node : adapter-static en mode SPA (fallback index.html).
// Voir https://v2.tauri.app/start/frontend/sveltekit/
import adapter from "@sveltejs/adapter-static";

import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// Config Vite pour Tauri (dev/build). Port 1430 pour cohabiter avec le desktop (1420).
export default defineConfig(async () => ({
  plugins: [
    sveltekit({
      preprocess: vitePreprocess(),
      adapter: adapter({ fallback: "index.html" })
    })
  ],

  // 1. ne pas masquer les erreurs Rust
  clearScreen: false,
  // 2. Tauri attend un port fixe (échoue s'il est pris). Sur device/émulateur Android,
  //    `tauri android dev` renseigne TAURI_DEV_HOST → HMR via l'IP de la machine.
  server: {
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1431,
        }
      : undefined,
    watch: {
      // 3. ne pas surveiller src-tauri
      ignored: ["**/src-tauri/**"],
    },
  },
}));
