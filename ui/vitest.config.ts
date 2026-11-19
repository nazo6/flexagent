import solid from "@solidjs/vite-plugin";
import path from "node:path";
import Icons from "unplugin-icons/vite";
import { defineConfig } from "vitest/config";

const libRoot = path.resolve(import.meta.dirname, "src/lib");

export default defineConfig({
  // `~icons/*` (unplugin-icons) is resolved by the app's Vite config; the test
  // config does not inherit it, so register the same plugin here.
  plugins: [solid(), Icons({ compiler: "solid" })],
  resolve: {
    alias: [{ find: /^#lib\//, replacement: `${libRoot}/` }],
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["src/test/setup.ts"],
    restoreMocks: true,
  },
});
