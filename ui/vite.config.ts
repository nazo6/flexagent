import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import type { ClientRequest } from "node:http";
import { defineConfig, type ProxyOptions } from "vite";

/** 開発時の API プロキシ先 (既定: ローカルノードの `fxg daemon`)。 */
const proxyTarget = process.env.FXG_DEV_PROXY ?? "http://127.0.0.1:7860";

/**
 * `fxg` の共通ミドルウェアは Host / Origin を検証する (DNS Rebinding / CSWSH
 * 対策)。プロキシでは Host を書き換え (`changeOrigin`)、Origin もプロキシ先
 * オリジンへ揃えて検証を通過させる (開発専用の設定)。
 */
const rewriteOrigin = (proxyReq: ClientRequest) => {
  proxyReq.setHeader("origin", proxyTarget);
};

const apiProxy: ProxyOptions = {
  target: proxyTarget,
  changeOrigin: true,
  ws: true,
  configure(proxy) {
    proxy.on("proxyReq", rewriteOrigin);
    proxy.on("proxyReqWs", rewriteOrigin);
  },
};

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": apiProxy,
    },
  },
});
