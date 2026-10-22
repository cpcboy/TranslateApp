import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// 不引 @types/node：这里只需要一个环境变量。
declare const process: { env: Record<string, string | undefined> };

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },

  // D1 只有纯函数单测，跑在 node 环境即可；D4 需要 DOM 时再切 jsdom。
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
