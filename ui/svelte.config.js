import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // SPA モード: 全ページをクライアントレンダリングし、index.html へフォールバックする
    // (`rust-embed` で `ui/build` を `fxg` バイナリへ同梱して配信する)
    adapter: adapter({ fallback: "index.html" }),
    // PWA: `src/service-worker.ts` を登録する (App Shell キャッシュ + Web Push)
    serviceWorker: { register: true },
  },
};

export default config;
