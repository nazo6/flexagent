import solid from "@solidjs/vite-plugin";
import path from "node:path";
import { defineConfig } from "vitest/config";

const libRoot = path.resolve(import.meta.dirname, "src/lib");

export default defineConfig({
  plugins: [solid()],
  resolve: {
    alias: [{ find: /^#lib\//, replacement: `${libRoot}/` }],
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
