import { Show, createEffect, createMemo, createSignal } from "solid-js";
import { languageForPath } from "#lib/code-languages.ts";
import type { FileDiff } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { errorMessage } from "#lib/api/error-messages.ts";

export interface MonacoDiffProps {
  file: FileDiff;
}

/** Side-by-side Monaco diff editor (desktop). Loaded lazily on demand. */
export default function MonacoDiff(props: MonacoDiffProps) {
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [container, setContainer] = createSignal<HTMLDivElement>();

  /** Editor inputs; an equal snapshot keeps the Monaco instance alive. */
  const source = createMemo(
    () => ({
      element: container(),
      path: props.file.path,
      oldText: props.file.old_text ?? "",
      newText: props.file.new_text ?? "",
    }),
    {
      equals: (left, right) =>
        left.element === right.element &&
        left.path === right.path &&
        left.oldText === right.oldText &&
        left.newText === right.newText,
    },
  );

  createEffect(source, (current) => {
    const element = current.element;
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

        const language = languageForPath(current.path);
        models = [
          monaco.editor.createModel(current.oldText, language),
          monaco.editor.createModel(current.newText, language),
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
        setError(errorMessage(cause));
        setLoading(false);
      }
    })();

    return () => {
      disposed = true;
      editor?.dispose();
      for (const model of models) model.dispose();
    };
  });

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
