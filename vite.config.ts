import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// @ts-expect-error process は Node.js のグローバル(@types/node を入れていないため)
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],

  // Tauri向けの設定(`tauri dev` / `tauri build` で使われる)
  // 1. Rustのエラーをviteが隠さないようにする
  clearScreen: false,
  // 2. Tauriは固定ポートを前提にするため、使用中なら失敗させる
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. src-tauri は監視しない
      ignored: ["**/src-tauri/**"],
    },
  },
});
