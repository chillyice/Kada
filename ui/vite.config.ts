import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// 端口对齐 src-tauri/tauri.conf.json 的 devUrl
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});