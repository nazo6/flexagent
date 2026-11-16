<script lang="ts">
  import type { FileDiff } from "#lib/generated/bindings.ts";
  import { languageForPath } from "#lib/code-languages.ts";
  import { m } from "#lib/paraglide/messages.js";

  let { file }: { file: FileDiff } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  $effect(() => {
    const element = container;
    // Read the props before the await so switching files re-runs this effect.
    const path = file.path;
    const oldText = file.old_text ?? "";
    const newText = file.new_text ?? "";
    if (element === null) return;
    let disposed = false;
    let editor: import("monaco-editor").editor.IStandaloneDiffEditor | null = null;
    let models: import("monaco-editor").editor.ITextModel[] = [];
    loading = true;
    error = null;

    void (async () => {
      try {
        // Import only the editor core and the basic-languages tokenizers: the
        // full `monaco-editor` entry also bundles the TypeScript / CSS / HTML /
        // JSON language services and their workers, which a read-only diff
        // viewer does not need.
        const monaco = await import("monaco-editor/editor/editor.api");
        await import("monaco-editor/basic-languages/monaco.contribution");
        const EditorWorker = (await import("monaco-editor/editor/editor.worker?worker")).default;
        (self as unknown as { MonacoEnvironment: { getWorker: () => Worker } }).MonacoEnvironment =
          {
            getWorker: () => new EditorWorker(),
          };
        if (disposed) return;

        const language = languageForPath(path);
        models = [
          monaco.editor.createModel(oldText, language),
          monaco.editor.createModel(newText, language),
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
        loading = false;
      } catch (cause) {
        if (disposed) return;
        error = cause instanceof Error ? cause.message : String(cause);
        loading = false;
      }
    })();

    return () => {
      disposed = true;
      editor?.dispose();
      for (const model of models) model.dispose();
    };
  });
</script>

<div class="relative min-h-40 overflow-hidden rounded-md border">
  <div bind:this={container} class="h-[60svh] w-full"></div>
  {#if error !== null}
    <p
      class="text-destructive bg-background/80 absolute inset-0 flex items-center justify-center p-4 text-center text-[13px]"
    >
      {m.diff_load_failed()}
    </p>
  {:else if loading}
    <p
      class="text-muted-foreground bg-background/80 absolute inset-0 flex items-center justify-center text-[13px]"
    >
      {m.diff_loading_editor()}
    </p>
  {/if}
</div>
