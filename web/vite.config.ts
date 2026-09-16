import path from "node:path"
import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
    // One React, always. Some dependencies ship a pre-bundled copy, and when
    // the bundler keeps both, hooks called from that copy run against a null
    // dispatcher: the app throws `null is not an object (evaluating
    // 'p.H.useState')` and renders an empty page. Chromium tolerated the
    // resulting chunk order, WebKit did not, so this surfaced as a blank
    // window in the desktop app while the browser looked fine.
    dedupe: ["react", "react-dom"],
  },
  // 仅 dev 生效：把 /api 转发到本地跑着的 Rust 后端（cargo run 或 docker
  // compose 都是 3000），这样 bun run dev 能直接调真实接口。
  server: {
    proxy: {
      "/api": {
        target: "http://127.0.0.1:3000",
        changeOrigin: true,
      },
    },
  },
})
