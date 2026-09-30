import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      // SvelteKit の `$lib` エイリアスを Vitest でも解決する
      $lib: new URL("./src/lib", import.meta.url).pathname,
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.{test,spec}.ts"],
    restoreMocks: true,
  },
});
