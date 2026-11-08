/**
 * Code highlighting with shiki (shared by Unified Diff / Markdown code fences).
 *
 * `shiki` itself is heavy, so it is lazy-loaded via dynamic import and the
 * highlighter is shared. Markdown code fences have no fixed language, so load
 * only the languages actually needed (unknown languages fall back to plain
 * text).
 */

type ShikiModule = typeof import("shiki");

interface SharedHighlighter {
  codeToHtml(code: string, options: { lang: string; theme: string }): string;
  getLoadedLanguages(): string[];
  loadLanguage(...langs: unknown[]): Promise<void>;
}

let shikiPromise: Promise<ShikiModule> | null = null;
let highlighterPromise: Promise<SharedHighlighter> | null = null;

/** Lazy-load the shiki module. */
function getShiki(): Promise<ShikiModule> {
  shikiPromise ??= import("shiki");
  return shikiPromise;
}

/** Lazily create shiki's shared highlighter. */
export function getSharedHighlighter(): Promise<SharedHighlighter> {
  highlighterPromise ??= (async () => {
    const shiki = await getShiki();
    return shiki.createHighlighter({
      themes: [shiki.bundledThemes["github-light"], shiki.bundledThemes["github-dark"]],
      langs: [shiki.bundledLanguages["diff"]],
    }) as unknown as SharedHighlighter;
  })();
  return highlighterPromise;
}

/** Whether the system color scheme is dark (shares the same check as the UI). */
export function prefersDarkMode(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Convert a Unified Diff into highlighted HTML. */
export async function highlightUnifiedDiff(diff: string, dark: boolean): Promise<string> {
  const highlighter = await getSharedHighlighter();
  return highlighter.codeToHtml(diff, {
    lang: "diff",
    theme: dark ? "github-dark" : "github-light",
  });
}

/** Resolve a Markdown language name (including aliases) to a shiki bundled definition. */
function resolveBundledLanguage(
  shiki: ShikiModule,
  lang: string,
): { id: string; input: unknown } | null {
  const bundled = shiki.bundledLanguages as Record<string, unknown>;
  if (bundled[lang] !== undefined) return { id: lang, input: bundled[lang] };
  const info = shiki.bundledLanguagesInfo.find(
    (entry) => entry.id === lang || entry.aliases?.includes(lang) === true,
  );
  if (info === undefined) return null;
  return { id: info.id, input: bundled[info.id] };
}

/** Convert one code block into highlighted HTML (`null` for unknown languages). */
export async function highlightCodeBlock(
  code: string,
  lang: string,
  dark: boolean,
): Promise<string | null> {
  const shiki = await getShiki();
  const resolved = resolveBundledLanguage(shiki, lang);
  if (resolved === null) return null;
  const highlighter = await getSharedHighlighter();
  if (!highlighter.getLoadedLanguages().includes(resolved.id)) {
    await highlighter.loadLanguage(resolved.input);
  }
  return highlighter.codeToHtml(code, {
    lang: resolved.id,
    theme: dark ? "github-dark" : "github-light",
  });
}

/** Cache limit for converted results (avoid re-converting stable blocks while streaming). */
const CODE_CACHE_LIMIT = 128;
const codeHtmlCache = new Map<string, string>();

/**
 * Replace each `pre > code.language-*` under `root` with shiki-highlighted HTML.
 * Called every time `{@html}` re-renders and recreates the DOM, so blocks with
 * identical content are reused from the cache.
 */
export async function enhanceCodeBlocks(root: ParentNode): Promise<void> {
  const dark = prefersDarkMode();
  const blocks = Array.from(root.querySelectorAll<HTMLElement>("pre > code[class*='language-']"));
  for (const code of blocks) {
    const pre = code.parentElement;
    if (pre === null) continue;
    const lang = /language-([\w+#.-]+)/.exec(code.className)?.[1] ?? "text";
    const source = code.textContent ?? "";
    const key = `${dark ? "d" : "l"}\u0000${lang}\u0000${source}`;
    let html = codeHtmlCache.get(key);
    if (html === undefined) {
      html = (await highlightCodeBlock(source, lang, dark)) ?? "";
      if (codeHtmlCache.size >= CODE_CACHE_LIMIT) codeHtmlCache.clear();
      codeHtmlCache.set(key, html);
    }
    // Keep marked's (escaped) output when highlighting is impossible, e.g. unknown languages
    if (html === "") continue;
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const replacement = template.content.firstElementChild;
    if (replacement !== null) pre.replaceWith(replacement);
  }
}
