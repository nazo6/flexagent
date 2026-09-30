<script lang="ts">
  import type { FileDiff } from '$lib/generated/FileDiff';

  let { file }: { file: FileDiff } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let loading = $state(true);

  const LANGUAGES: Record<string, string> = {
    ts: 'typescript',
    tsx: 'typescript',
    mts: 'typescript',
    cts: 'typescript',
    js: 'javascript',
    jsx: 'javascript',
    mjs: 'javascript',
    cjs: 'javascript',
    json: 'json',
    md: 'markdown',
    css: 'css',
    scss: 'scss',
    less: 'less',
    html: 'html',
    svelte: 'html',
    vue: 'html',
    rs: 'rust',
    py: 'python',
    toml: 'ini',
    ini: 'ini',
    yml: 'yaml',
    yaml: 'yaml',
    sh: 'shell',
    bash: 'shell',
    zsh: 'shell',
    ps1: 'powershell',
    sql: 'sql',
    go: 'go',
    c: 'c',
    h: 'c',
    cc: 'cpp',
    cpp: 'cpp',
    hpp: 'cpp',
    java: 'java',
    kt: 'kotlin',
    swift: 'swift',
    rb: 'ruby',
    php: 'php',
    xml: 'xml'
  };

  function languageFor(path: string): string {
    const extension = path.split('.').pop()?.toLowerCase() ?? '';
    return LANGUAGES[extension] ?? 'plaintext';
  }

  $effect(() => {
    const element = container;
    if (element === null) return;
    let disposed = false;
    let editor: import('monaco-editor').editor.IStandaloneDiffEditor | null = null;
    let models: import('monaco-editor').editor.ITextModel[] = [];

    void (async () => {
      const monaco = await import('monaco-editor');
      const EditorWorker = (
        await import('monaco-editor/editor/editor.worker?worker')
      ).default;
      (
        self as unknown as { MonacoEnvironment: { getWorker: () => Worker } }
      ).MonacoEnvironment = { getWorker: () => new EditorWorker() };
      if (disposed) return;

      const language = languageFor(file.path);
      models = [
        monaco.editor.createModel(file.old_text ?? '', language),
        monaco.editor.createModel(file.new_text ?? '', language)
      ];
      editor = monaco.editor.createDiffEditor(element, {
        automaticLayout: true,
        readOnly: true,
        originalEditable: false,
        renderSideBySide: true,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        fontSize: 12
      });
      editor.setModel({ original: models[0], modified: models[1] });
      loading = false;
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
  {#if loading}
    <p class="text-muted-foreground bg-background/80 absolute inset-0 flex items-center justify-center text-xs">
      Diff エディタを読み込み中…
    </p>
  {/if}
</div>
