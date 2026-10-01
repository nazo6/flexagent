/**
 * shiki によるコードハイライト (Unified Diff / Markdown コードフェンス共用)。
 *
 * `shiki` 本体は重いため動的 import で遅延ロードし、ハイライターは共有する。
 * Markdown のコードフェンスは言語が不定のため、必要になった言語だけを
 * 追加ロードする (未知の言語はプレーン表示へフォールバック)。
 */

type ShikiModule = typeof import("shiki");

interface SharedHighlighter {
  codeToHtml(code: string, options: { lang: string; theme: string }): string;
  getLoadedLanguages(): string[];
  loadLanguage(...langs: unknown[]): Promise<void>;
}

let shikiPromise: Promise<ShikiModule> | null = null;
let highlighterPromise: Promise<SharedHighlighter> | null = null;

/** shiki 本体を遅延ロードする。 */
function getShiki(): Promise<ShikiModule> {
  shikiPromise ??= import("shiki");
  return shikiPromise;
}

/** shiki の共有ハイライターを遅延生成する。 */
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

/** システムの配色設定が dark か (UI 本体と同じ判定を共有する)。 */
export function prefersDarkMode(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Unified Diff をハイライト済み HTML に変換する。 */
export async function highlightUnifiedDiff(diff: string, dark: boolean): Promise<string> {
  const highlighter = await getSharedHighlighter();
  return highlighter.codeToHtml(diff, {
    lang: "diff",
    theme: dark ? "github-dark" : "github-light",
  });
}

/** Markdown の言語名 (エイリアス含む) を shiki のバンドル定義へ解決する。 */
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

/** コードブロック 1 つをハイライト済み HTML へ変換する (未知言語は `null`)。 */
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

/** 変換結果のキャッシュ上限 (ストリーミング中でも安定ブロックを再変換しない)。 */
const CODE_CACHE_LIMIT = 128;
const codeHtmlCache = new Map<string, string>();

/**
 * `root` 配下の `pre > code.language-*` を shiki のハイライト済み HTML で
 * 置き換える。`{@html}` の再描画で DOM が作り直されるたびに呼ばれるため、
 * 内容が同じブロックはキャッシュで再利用する。
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
    // 未知言語などハイライト不能な場合は marked の出力 (エスケープ済み) を残す
    if (html === "") continue;
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const replacement = template.content.firstElementChild;
    if (replacement !== null) pre.replaceWith(replacement);
  }
}
