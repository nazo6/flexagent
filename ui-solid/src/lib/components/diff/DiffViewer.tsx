import { Show, createEffect, createMemo, createSignal } from "solid-js";
import type { FileDiff } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import MonacoDiff from "./MonacoDiff.tsx";
import ShikiDiff from "./ShikiDiff.tsx";

export interface DiffViewerProps {
  file: FileDiff | null;
}

/** One file diff: Monaco side-by-side on desktop, a shiki unified diff on mobile. */
export default function DiffViewer(props: DiffViewerProps) {
  const [viewportWidth, setViewportWidth] = createSignal(
    typeof window === "undefined" ? 0 : window.innerWidth,
  );

  createEffect(
    () => typeof window !== "undefined",
    (enabled) => {
      if (!enabled) return;
      const update = () => setViewportWidth(window.innerWidth);
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    },
  );

  const canUseMonaco = createMemo(() => {
    const file = props.file;
    return (
      viewportWidth() >= 768 && file !== null && file.old_text !== null && file.new_text !== null
    );
  });

  return (
    <Show
      when={props.file}
      fallback={
        <p class="text-muted-foreground flex h-full items-center justify-center text-sm">
          {m.diff_select_file()}
        </p>
      }
    >
      {(file) => (
        <div class="flex flex-col gap-2">
          <div class="flex flex-wrap items-center gap-2 text-[13px]">
            <span class="font-mono break-all">{file().path}</span>
            <span class="text-ok">+{file().additions}</span>
            <span class="text-danger">-{file().deletions}</span>
          </div>
          <Show
            when={canUseMonaco()}
            fallback={
              <Show
                when={file().unified_diff !== ""}
                fallback={
                  <p class="text-muted-foreground text-sm">{m.diff_binary_or_unsupported()}</p>
                }
              >
                <ShikiDiff diff={file().unified_diff} />
              </Show>
            }
          >
            <MonacoDiff file={file()} />
          </Show>
        </div>
      )}
    </Show>
  );
}
