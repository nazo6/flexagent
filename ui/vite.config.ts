import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    // 開発時はローカルノード (`fxg daemon`) へ API をプロキシする
    port: 5173,
    strictPort: true,
  },
});
