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
    // Test the browser build of Svelte: runes (`$effect` / `flushSync`) are no-ops
    // in the server build, so reactivity tests would silently observe nothing.
    conditions: ["browser"],
    alias: {
      // Resolve SvelteKit's `#lib` alias in Vitest as well
      $lib: new URL("./src/lib", import.meta.url).pathname,
      "$app/state": new URL("./src/test/app-state-stub.ts", import.meta.url).pathname,
      "$app/navigation": new URL("./src/test/app-navigation-stub.ts", import.meta.url).pathname,
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.{test,spec}.ts", "src/**/*.{test,spec}.svelte.ts"],
    restoreMocks: true,
  },
});
