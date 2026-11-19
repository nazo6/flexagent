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
 * Source modules the service worker shares with the app.
 *
 * The worker is emitted as one classic script (not a module), so the build
 * strips the imports, checks that each requested name is exported, drops the
 * TypeScript-only type syntax, and appends the bodies. The list comes from the
 * worker's own `import` statements, so it cannot drift from the code that uses
 * them.
 */
const SERVICE_WORKER_SHARED = path.join(import.meta.dirname, "src/service-worker/format.ts");

/**
 * Remove the TypeScript syntax of a dependency-free function body: explicit
 * parameter types (`(x: string)`), return types (`): Uint8Array<ArrayBuffer> {`)
 * and `as` assertions. The shared file keeps its types checked by `tsc`; only
 * the emitted `sw.js` needs plain JavaScript.
 */
function stripTypeSyntax(source: string): string {
  return source
    .replace(/:\s*[A-Za-z_$][\w$]*(?:<[^<>()]*>)?(?:\[\])*(?=\s*[,)=;{])/g, "")
    .replace(/\s+as\s+[A-Za-z_$][\w$]*(?:<[^<>()]*>)?(?:\[\])*/g, "");
}

/** Body of one exported function in a shared module. */
function sharedFunctionSource(source: string, name: string, file: string): string {
  const start = source.search(new RegExp(`export function ${name}\\(`));
  if (start < 0) throw new Error(`${file} does not export \`${name}\``);
  const open = source.indexOf("{", source.indexOf(")", start));
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    else if (source[index] === "}") {
      depth -= 1;
      if (depth === 0)
        return stripTypeSyntax(source.slice(start, index + 1)).replace(/^export /, "");
    }
  }
  throw new Error(`${file}: unbalanced braces in \`${name}\``);
}

/**
 * Expand the shared `import { a, b } from "..."` lines of the worker into the
 * function bodies they name. The worker has no module loader, and a module
 * worker would break on browsers that only register classic workers.
 */
function inlineServiceWorkerImports(source: string): string {
  const shared = fs.readFileSync(SERVICE_WORKER_SHARED, "utf8");
  return source.replace(/^import\s*\{([^}]*)\}\s*from\s*"[^"]*";?$/gm, (_line, names: string) => {
    return names
      .split(",")
      .map((name) => name.trim())
      .filter((name) => name !== "")
      .map((name) => sharedFunctionSource(shared, name, "src/service-worker/format.ts"))
      .join("\n\n");
  });
}

/**
 * Emits `sw.js` after the build: injects a fresh cache version and the list of
 * every emitted file, so the service worker can precache the app shell.
 */
function serviceWorkerPlugin(): Plugin {
  return {
    name: "fxg-service-worker",
    apply: "build",
    buildStart() {
      this.addWatchFile(SERVICE_WORKER_SHARED);
    },
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
      const template = fs.readFileSync(
        path.join(import.meta.dirname, "src/service-worker.js"),
        "utf8",
      );
      const source = inlineServiceWorkerImports(template)
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
