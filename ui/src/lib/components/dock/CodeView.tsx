import { Show, createEffect, createSignal } from "solid-js";
import { languageForPath } from "#lib/code-languages.ts";
import { FOCUS_LINE_CLASS, highlightCodeBlock, isDarkMode } from "#lib/highlight.ts";

export interface CodeViewProps {
  path: string;
  text: string;
  /** 1-based line to focus, or `null`. */
  line?: number | null;
}

/** Read-only source view with shiki highlighting and an optional focused line. */
export default function CodeView(props: CodeViewProps) {
  const [html, setHtml] = createSignal<string | null>(null);
  const [scroller, setScroller] = createSignal<HTMLDivElement | null>(null);

  createEffect(
    () => ({
      path: props.path,
      text: props.text,
      line: props.line ?? null,
      dark: isDarkMode(),
    }),
    (source) => {
      let disposed = false;
      setHtml(null);
      void (async () => {
        try {
          const rendered = await highlightCodeBlock(
            source.text,
            languageForPath(source.path),
            source.dark,
            source.line,
          );
          if (!disposed) setHtml(rendered);
        } catch {
          if (!disposed) setHtml(null);
        }
      })();
      return () => {
        disposed = true;
      };
    },
  );

  // Center the requested line without scrolling the surrounding page. Shiki
  // marks it with `FOCUS_LINE_CLASS` while building the highlighted HTML.
  createEffect(
    () => ({ node: scroller(), html: html() }),
    ({ node, html: current }) => {
      if (node === null || current === null) return;
      const frame = requestAnimationFrame(() => {
        const target = node.querySelector<HTMLElement>(`.${FOCUS_LINE_CLASS}`);
        if (target === null) return;
        const targetRect = target.getBoundingClientRect();
        const nodeRect = node.getBoundingClientRect();
        node.scrollTop +=
          targetRect.top - nodeRect.top - (node.clientHeight - targetRect.height) / 2;
      });
      return () => cancelAnimationFrame(frame);
    },
  );

  return (
    <Show
      when={html() !== null}
      fallback={
        <pre class="bg-surface-raised h-full overflow-auto rounded-md border p-2 font-mono text-[12px] leading-relaxed">
          <code>{props.text}</code>
        </pre>
      }
    >
      <div
        ref={setScroller}
        class="h-full overflow-auto rounded-md border text-[12px] [&_code]:block [&_code]:min-w-max [&_pre]:w-max [&_pre]:min-w-full [&_pre]:p-2 [&_pre]:font-mono [&_pre]:leading-relaxed [&_.fxg-line-focus]:inline-block [&_.fxg-line-focus]:w-full [&_.fxg-line-focus]:bg-signal-soft/60"
        innerHTML={html() ?? ""}
      />
    </Show>
  );
}
