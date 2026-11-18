import { paraglideVitePlugin } from "@inlang/paraglide-js";
import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import fs from "node:fs";
import path from "node:path";
import Icons from "unplugin-icons/vite";
import { defineConfig, type Plugin, type ProxyOptions } from "vite";
import type { ClientRequest } from "node:http";

/**
 * API proxy target in development (default: the local `fxg daemon` node).
 *
 * The shared middleware validates Host / Origin (DNS rebinding and CSWSH
 * protection), so the proxy rewrites Host and aligns Origin with the target.
 */
const proxyTarget = process.env.FXG_DEV_PROXY ?? "http://127.0.0.1:7860";

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

const libRoot = path.resolve(import.meta.dirname, "src/lib");

/**
 * Emits `sw.js` after the build: injects a fresh cache version and the list of
 * every emitted file, so the service worker can precache the app shell.
 */
function serviceWorkerPlugin(): Plugin {
  return {
    name: "fxg-service-worker",
    apply: "build",
    closeBundle() {
      const outDir = path.resolve(import.meta.dirname, "build");
      const files: string[] = [];
      const walk = (dir: string, prefix: string) => {
        for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
          const url = `${prefix}/${entry.name}`;
          if (entry.isDirectory()) walk(path.join(dir, entry.name), url);
          else files.push(url);
        }
      };
      walk(outDir, "");
      const precache = files.filter((file) => file !== "/sw.js");
      const version = Date.now().toString(36);
      const source = fs
        .readFileSync(path.join(import.meta.dirname, "src/service-worker.js"), "utf8")
        .replace("__FXG_VERSION__", JSON.stringify(version))
        .replace("__FXG_PRECACHE__", JSON.stringify(precache));
      fs.writeFileSync(path.join(outDir, "sw.js"), source);
    },
  };
}

export default defineConfig({
  plugins: [
    tailwindcss(),
    solid(),
    Icons({ compiler: "solid" }),
    serviceWorkerPlugin(),
    paraglideVitePlugin({
      project: "./project.inlang",
      outdir: "./src/lib/paraglide",
      emitTsDeclarations: true,
      strategy: ["baseLocale"],
      outputStructure: "locale-modules",
    }),
  ],
  resolve: {
    alias: [{ find: /^#lib\//, replacement: `${libRoot}/` }],
  },
  build: {
    // `rust-embed` embeds `ui/build` into the fxg binary.
    outDir: "build",
    chunkSizeWarningLimit: 3000,
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": apiProxy,
    },
  },
  preview: {
    port: 4173,
    strictPort: true,
    proxy: {
      "/api": apiProxy,
    },
  },
});
