<script lang="ts">
  import CodeView from "#lib/components/dock/CodeView.svelte";
  import GitStatusBadge from "#lib/components/dock/GitStatusBadge.svelte";
  import DiffViewer from "#lib/components/diff/DiffViewer.svelte";
  import MarkdownText from "#lib/components/MarkdownText.svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import type {
    DockContext,
    DockDiffScope,
    DockFileMode,
    DockFileTab,
  } from "#lib/stores/dock.svelte.ts";
  import { dock } from "#lib/stores/dock.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import type {
    WorkspaceDiffResponse,
    WorkspaceFileView,
    WorkspaceGitFileStatus,
  } from "#lib/generated/bindings.ts";
  import { formatBytes } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import ChevronLeftIcon from "@lucide/svelte/icons/chevron-left";
  import EyeIcon from "@lucide/svelte/icons/eye";
  import FileCodeIcon from "@lucide/svelte/icons/file-code";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  interface Props {
    ctx: DockContext;
    tab: DockFileTab;
    mobile?: boolean;
    /** Mobile: show the tree again. */
    onback: () => void;
  }

  let { ctx, tab, mobile = false, onback }: Props = $props();

  const remote = $derived(sync.connection.role === "hub");
  const path = $derived(tab.path);
  const mode = $derived<DockFileMode>(tab.mode);
  const scope = $derived<DockDiffScope>(tab.scope);
  const isMarkdown = $derived(/\.(md|markdown|mdx)$/i.test(path));

  let markdownPreview = $state(true);
  let view = $state<WorkspaceFileView | null>(null);
  let diff = $state<WorkspaceDiffResponse | null>(null);
  let viewLoading = $state(false);
  let diffLoading = $state(false);
  let viewError = $state<string | null>(null);
  let diffError = $state<string | null>(null);
  let fileStatus = $state<WorkspaceGitFileStatus | null>(null);
  let refreshToken = $state(0);
  let viewGeneration = 0;
  let diffGeneration = 0;
  let statusGeneration = 0;

  const selectedDiff = $derived(diff?.files.at(0) ?? null);

  $effect(() => {
    const currentPath = path;
    const currentMode = mode;
    const currentScope = scope;
    const token = refreshToken;
    void token;
    if (currentPath === "" || remote) return;
    void loadFileStatus(currentPath);
    if (currentMode === "view") {
      void loadView(currentPath);
    } else {
      void loadDiff(currentPath, currentScope);
    }
  });

  async function loadFileStatus(filePath: string): Promise<void> {
    const token = ++statusGeneration;
    try {
      const result = await sync.connection.client.workspaceGitStatus(ctx.nodeId, ctx.localPath);
      if (token !== statusGeneration) return;
      fileStatus = result.entries.find((entry) => entry.absolute_path === filePath)?.status ?? null;
    } catch {
      if (token === statusGeneration) fileStatus = null;
    }
  }

  async function loadView(filePath: string): Promise<void> {
    const token = ++viewGeneration;
    viewLoading = true;
    viewError = null;
    try {
      const result = await sync.connection.client.workspaceFsFile(ctx.nodeId, filePath);
      if (token !== viewGeneration) return;
      view = result;
    } catch (cause) {
      if (token !== viewGeneration) return;
      view = null;
      viewError = errorMessage(cause);
    } finally {
      if (token === viewGeneration) viewLoading = false;
    }
  }

  async function loadDiff(filePath: string, diffScope: DockDiffScope): Promise<void> {
    const token = ++diffGeneration;
    diffLoading = true;
    diffError = null;
    try {
      const result = await sync.connection.client.workspaceGitDiff(ctx.nodeId, ctx.localPath, {
        scope: diffScope,
        file: filePath,
      });
      if (token !== diffGeneration) return;
      diff = result;
    } catch (cause) {
      if (token !== diffGeneration) return;
      diff = null;
      diffError = errorMessage(cause);
    } finally {
      if (token === diffGeneration) diffLoading = false;
    }
  }

  function refresh(): void {
    refreshToken += 1;
  }
</script>

<div class="border-border flex h-9 shrink-0 items-center gap-1 border-b px-1.5">
  {#if mobile}
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
      title={m.dock_files_title()}
      aria-label={m.dock_files_title()}
      onclick={onback}
    >
      <ChevronLeftIcon class="size-4" />
    </button>
  {/if}

  <div class="bg-muted flex items-center rounded-md p-0.5">
    {#each ["view", "diff"] as candidate (candidate)}
      <button
        type="button"
        class={cn(
          "rounded-sm px-2 py-0.5 text-[11px] font-medium transition-colors",
          mode === candidate
            ? "bg-card text-foreground shadow-sm"
            : "text-muted-foreground hover:text-foreground",
        )}
        aria-pressed={mode === candidate}
        onclick={() => dock.setFileMode(tab.path, candidate as DockFileMode)}
      >
        {candidate === "view" ? m.dock_file_view() : m.dock_file_diff()}
      </button>
    {/each}
  </div>

  {#if mode === "diff"}
    <span class="text-muted-foreground shrink-0 text-[10px] font-medium tracking-wide uppercase">
      {scope === "branch_base" ? m.dock_changes_scope_branch() : m.dock_changes_scope_worktree()}
    </span>
  {:else if isMarkdown}
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
      title={markdownPreview ? m.dock_file_source() : m.dock_file_preview()}
      aria-label={markdownPreview ? m.dock_file_source() : m.dock_file_preview()}
      onclick={() => (markdownPreview = !markdownPreview)}
    >
      {#if markdownPreview}
        <FileCodeIcon class="size-3.5" />
      {:else}
        <EyeIcon class="size-3.5" />
      {/if}
    </button>
  {/if}

  <button
    type="button"
    class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
    title={m.dock_files_refresh()}
    aria-label={m.dock_files_refresh()}
    onclick={refresh}
  >
    <RefreshCwIcon class="size-3.5" />
  </button>

  <span class="ml-auto flex shrink-0 items-center">
    {#if fileStatus !== null}
      <GitStatusBadge status={fileStatus} />
    {/if}
  </span>
</div>

{#if remote}
  <div class="flex flex-1 items-center justify-center p-6 text-center">
    <p class="text-muted-foreground text-[12px]">{m.dock_files_remote_disabled()}</p>
  </div>
{:else if mode === "view"}
  {#if viewLoading}
    <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-[12px]">
      <LoaderCircleIcon class="size-3.5 animate-spin" />
      {m.dock_file_loading()}
    </div>
  {:else if viewError !== null}
    <p class="text-destructive flex flex-1 items-center justify-center p-4 text-center text-[12px]">
      {viewError}
    </p>
  {:else if view !== null}
    <div class="min-h-0 flex-1 overflow-hidden p-2">
      {#if view.content.kind === "text"}
        {#if isMarkdown && markdownPreview}
          <div class="app-scroll h-full overflow-y-auto rounded-md border p-4">
            <MarkdownText text={view.content.text} />
          </div>
        {:else}
          <CodeView path={view.path} text={view.content.text} />
        {/if}
      {:else if view.content.kind === "image"}
        <div class="flex h-full items-center justify-center overflow-auto rounded-md border p-2">
          <img
            src={`data:${view.content.mime};base64,${view.content.data_b64}`}
            alt={path}
            class="max-h-full max-w-full object-contain"
          />
        </div>
      {:else if view.content.kind === "binary"}
        <p
          class="text-muted-foreground flex h-full items-center justify-center p-4 text-center text-[12px]"
        >
          {m.dock_file_binary()}
        </p>
      {:else}
        <p
          class="text-muted-foreground flex h-full items-center justify-center p-4 text-center text-[12px]"
        >
          {m.dock_file_too_large({ size: formatBytes(view.content.size) })}
        </p>
      {/if}
    </div>
  {/if}
{:else}
  {#if diffLoading}
    <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-[12px]">
      <LoaderCircleIcon class="size-3.5 animate-spin" />
      {m.diff_loading()}
    </div>
  {:else if diffError !== null}
    <p class="text-destructive flex flex-1 items-center justify-center p-4 text-center text-[12px]">
      {diffError}
    </p>
  {:else if selectedDiff === null}
    <p
      class="text-muted-foreground flex flex-1 items-center justify-center p-4 text-center text-[12px]"
    >
      {m.dock_file_no_changes()}
    </p>
  {:else}
    <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-2">
      <DiffViewer file={selectedDiff} />
    </div>
  {/if}
{/if}
