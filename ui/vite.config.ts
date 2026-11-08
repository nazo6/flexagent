import { paraglideVitePlugin } from "@inlang/paraglide-js";
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import type { ClientRequest } from "node:http";
import { defineConfig, type ProxyOptions } from "vite";

/** API proxy target in development (default: the local `fxg daemon` node). */
const proxyTarget = process.env.FXG_DEV_PROXY ?? "http://127.0.0.1:7860";

/**
 * `fxg`'s shared middleware validates the Host / Origin headers (DNS rebinding
 * and CSWSH protection). The proxy rewrites Host (`changeOrigin`) and aligns
 * Origin with the proxy target so validation passes (development only).
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
  plugins: [
    tailwindcss(),
    sveltekit({
      preprocess: vitePreprocess(),
      // SPA mode: render every page on the client and fall back to index.html
      // (`rust-embed` embeds `ui/build` into the `fxg` binary)
      adapter: adapter({ fallback: "index.html" }),

      // PWA: register `src/service-worker.ts` (App Shell cache + Web Push)
      serviceWorker: { register: true },
    }),
    // i18n: messages live in `messages/{locale}.json` and compile to
    // `src/lib/paraglide` (generated, Git-ignored).
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/lib/paraglide",
      emitTsDeclarations: true,
      strategy: ["baseLocale"],
    }),
  ],
  optimizeDeps: {
    // lucide has a very large number of per-icon deep imports, so lazy
    // discovery triggers dependency re-optimization (forced reload) each time;
    // exclude it from pre-bundling.
    exclude: ["@lucide/svelte"],
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": apiProxy,
    },
  },
});
