import { paraglideVitePlugin } from "@inlang/paraglide-js";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    svelte({ configFile: false }),
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/lib/paraglide",
      emitTsDeclarations: true,
      strategy: ["baseLocale"],
      outputStructure: "locale-modules",
    }),
  ],
  resolve: {
    alias: {
      // Resolve SvelteKit's `#lib` alias in Vitest as well
      $lib: new URL("./src/lib", import.meta.url).pathname,
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.{test,spec}.ts"],
    restoreMocks: true,
  },
});
