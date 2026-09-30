/**
 * shiki による Unified Diff ハイライト (モバイル / 非 Monaco 環境用)。
 *
 * `shiki` 本体は重いため動的 import で遅延ロードし、インスタンスは共有する。
 */

interface DiffHighlighter {
  codeToHtml(code: string, options: { lang: string; theme: string }): string;
}

let highlighterPromise: Promise<DiffHighlighter> | null = null;

/** shiki の共有ハイライターを遅延生成する。 */
export function getDiffHighlighter(): Promise<DiffHighlighter> {
  highlighterPromise ??= import("shiki").then(
    (shiki) =>
      shiki.createHighlighter({
        themes: [shiki.bundledThemes["github-light"], shiki.bundledThemes["github-dark"]],
        langs: [shiki.bundledLanguages["diff"]],
      }) as unknown as DiffHighlighter,
  );
  return highlighterPromise;
}

/** Unified Diff をハイライト済み HTML に変換する。 */
export async function highlightUnifiedDiff(diff: string, dark: boolean): Promise<string> {
  const highlighter = await getDiffHighlighter();
  return highlighter.codeToHtml(diff, {
    lang: "diff",
    theme: dark ? "github-dark" : "github-light",
  });
}
