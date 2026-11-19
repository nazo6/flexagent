import { Show, createEffect, createSignal } from "solid-js";
import { highlightUnifiedDiff, isDarkMode } from "#lib/highlight.ts";
import DiffText from "./DiffText.tsx";

export interface ShikiDiffProps {
  diff: string;
  maxHeight?: string;
}

/** Unified diff highlighted with shiki (mobile / no-Monaco path). */
export default function ShikiDiff(props: ShikiDiffProps) {
  const [html, setHtml] = createSignal<string | null>(null);
  const maxHeight = () => props.maxHeight ?? "60svh";

  createEffect(
    () => ({ text: props.diff, dark: isDarkMode() }),
    (source) => {
      let disposed = false;
      setHtml(null);
      void (async () => {
        try {
          const rendered = await highlightUnifiedDiff(source.text, source.dark);
          if (!disposed) setHtml(rendered);
        } catch {
          // Fall back to DiffText when highlighting fails
          if (!disposed) setHtml(null);
        }
      })();
      return () => {
        disposed = true;
      };
    },
  );

  return (
    <Show when={html() !== null} fallback={<DiffText diff={props.diff} maxHeight={maxHeight()} />}>
      <div
        class="overflow-auto rounded-md border text-[13px] [&_code]:block [&_code]:min-w-max [&_pre]:min-w-full [&_pre]:w-max [&_pre]:p-2 [&_pre]:font-mono [&_pre]:leading-relaxed"
        style={{ "max-height": maxHeight() }}
        innerHTML={html() ?? ""}
      />
    </Show>
  );
}
