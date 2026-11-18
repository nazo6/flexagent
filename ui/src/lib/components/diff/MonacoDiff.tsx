import { Show, createEffect, createSignal } from "solid-js";
import { languageForPath } from "#lib/code-languages.ts";
import type { FileDiff } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";

export interface MonacoDiffProps {
  file: FileDiff;
}

/** Side-by-side Monaco diff editor (desktop). Loaded lazily on demand. */
export default function MonacoDiff(props: MonacoDiffProps) {
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [container, setContainer] = createSignal<HTMLDivElement>();

  createEffect(
    () => ({
      element: container(),
      path: props.file.path,
      oldText: props.file.old_text ?? "",
      newText: props.file.new_text ?? "",
    }),
    (source) => {
      const element = source.element;
      if (element === undefined) return;
      let disposed = false;
      let editor: import("monaco-editor").editor.IStandaloneDiffEditor | null = null;
      let models: import("monaco-editor").editor.ITextModel[] = [];
      setLoading(true);
      setError(null);

      void (async () => {
        try {
          // Import only the editor core and the basic-languages tokenizers: the
          // full `monaco-editor` entry also bundles the TypeScript / CSS / HTML /
          // JSON language services and their workers, which a read-only diff
          // viewer does not need.
          const monaco = await import("monaco-editor/editor/editor.api");
          await import("monaco-editor/basic-languages/monaco.contribution");
          const EditorWorker = (await import("monaco-editor/editor/editor.worker?worker")).default;
          globalThis.MonacoEnvironment = {
            getWorker: () => new EditorWorker(),
          };
          if (disposed) return;

          const language = languageForPath(source.path);
          models = [
            monaco.editor.createModel(source.oldText, language),
            monaco.editor.createModel(source.newText, language),
          ];
          editor = monaco.editor.createDiffEditor(element, {
            automaticLayout: true,
            readOnly: true,
            originalEditable: false,
            renderSideBySide: true,
            minimap: { enabled: false },
            scrollBeyondLastLine: false,
            fontSize: 12,
          });
          editor.setModel({ original: models[0], modified: models[1] });
          setLoading(false);
        } catch (cause) {
          if (disposed) return;
          setError(cause instanceof Error ? cause.message : String(cause));
          setLoading(false);
        }
      })();

      return () => {
        disposed = true;
        editor?.dispose();
        for (const model of models) model.dispose();
      };
    },
  );

  return (
    <div class="relative min-h-40 overflow-hidden rounded-md border">
      <div ref={(element) => setContainer(() => element)} class="h-[60svh] w-full"></div>
      <Show when={error() !== null}>
        <p class="text-destructive bg-background/80 absolute inset-0 flex items-center justify-center p-4 text-center text-[13px]">
          {m.diff_load_failed()}
        </p>
      </Show>
      <Show when={error() === null && loading()}>
        <p class="text-muted-foreground bg-background/80 absolute inset-0 flex items-center justify-center text-[13px]">
          {m.diff_loading_editor()}
        </p>
      </Show>
    </div>
  );
}
