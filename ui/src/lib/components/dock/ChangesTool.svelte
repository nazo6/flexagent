<script lang="ts">
  import DiffPane from "#lib/components/diff/DiffPane.svelte";
  import GitStatusBadge from "#lib/components/dock/GitStatusBadge.svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { joinPath } from "#lib/dock/paths.ts";
  import type { DockContext, DockDiffScope } from "#lib/stores/dock.svelte.ts";
  import { dock } from "#lib/stores/dock.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import type {
    FileDiff,
    WorkspaceDiffResponse,
    WorkspaceGitFileStatus,
    WorkspaceGitStatusResponse,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  let { ctx, items }: { ctx: DockContext; items: TimelineItem[] } = $props();

  type Scope = "session" | "uncommitted" | "branch_base";

  const remote = $derived(sync.connection.role === "hub");

  let scope = $state<Scope>("session");
  let status = $state<WorkspaceGitStatusResponse | null>(null);
  let branchDiff = $state<WorkspaceDiffResponse | null>(null);
  let statusLoading = $state(false);
  let branchLoading = $state(false);
  let statusError = $state<string | null>(null);
  let branchError = $state<string | null>(null);
  let refreshToken = $state(0);
  let statusGeneration = 0;
  let branchGeneration = 0;

  const scopes: { value: Scope; label: string }[] = [
    { value: "session", label: m.dock_changes_scope_session() },
    { value: "uncommitted", label: m.dock_changes_scope_worktree() },
    { value: "branch_base", label: m.dock_changes_scope_branch() },
  ];

  /** Files edited by this session's tool calls (last write wins). */
  const sessionFiles = $derived.by(() => {
    const byPath = new Map<string, FileDiff>();
    for (const item of items) {
      if (item.kind === "tool" && item.diff !== null) {
        byPath.set(item.diff.path, item.diff);
      }
    }
    return [...byPath.values()];
  });

  const workspaceRoot = $derived(status?.root_path ?? ctx.localPath);

  const statusByPath = $derived(
    new Map<string, WorkspaceGitFileStatus>(
      (status?.entries ?? []).map((entry) => [entry.absolute_path, entry.status]),
    ),
  );

  $effect(() => {
    const token = refreshToken;
    void token;
    if (remote) return;
    void loadStatus();
  });

  $effect(() => {
    const current = scope;
    const token = refreshToken;
    void token;
    if (remote || current !== "branch_base") return;
    void loadBranchDiff();
  });

  async function loadStatus(): Promise<void> {
    const token = ++statusGeneration;
    statusLoading = true;
    statusError = null;
    try {
      status = await sync.connection.client.workspaceGitStatus(ctx.nodeId, ctx.localPath);
    } catch (cause) {
      if (token !== statusGeneration) return;
      status = null;
      statusError = errorMessage(cause);
    } finally {
      if (token === statusGeneration) statusLoading = false;
    }
  }

  async function loadBranchDiff(): Promise<void> {
    const token = ++branchGeneration;
    branchLoading = true;
    branchError = null;
    try {
      branchDiff = await sync.connection.client.workspaceGitDiff(ctx.nodeId, ctx.localPath, {
        scope: "branch_base",
      });
    } catch (cause) {
      if (token !== branchGeneration) return;
      branchDiff = null;
      branchError = errorMessage(cause);
    } finally {
      if (token === branchGeneration) branchLoading = false;
    }
  }

  function refresh(): void {
    refreshToken += 1;
  }

  function openDiff(absolutePath: string, diffScope: DockDiffScope = "uncommitted"): void {
    dock.openFile(absolutePath, "diff", diffScope);
  }

  function sessionAbsolute(file: FileDiff): string {
    return joinPath(ctx.localPath, file.path);
  }

  function branchAbsolute(file: FileDiff): string {
    return joinPath(workspaceRoot, file.path);
  }
</script>

{#if remote}
  <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-3">
    <DiffPane sessionId={ctx.sessionId} {items} />
  </div>
{:else}
  <div class="border-border flex h-9 shrink-0 items-center gap-1 border-b px-1.5">
    <div class="bg-muted flex min-w-0 items-center rounded-md p-0.5">
      {#each scopes as option (option.value)}
        <button
          type="button"
          class={cn(
            "truncate rounded-sm px-2 py-0.5 text-[11px] font-medium transition-colors",
            scope === option.value
              ? "bg-card text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
          aria-pressed={scope === option.value}
          onclick={() => (scope = option.value)}
        >
          {option.label}
        </button>
      {/each}
    </div>
    <button
      type="button"
      class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
      title={m.dock_files_refresh()}
      aria-label={m.dock_files_refresh()}
      onclick={refresh}
    >
      <RefreshCwIcon class="size-3.5" />
    </button>
    {#if scope === "branch_base" && branchDiff !== null}
      <span class="text-muted-foreground ml-auto truncate font-mono text-[11px]">
        {branchDiff.base_branch ?? branchDiff.scope}
        {#if branchDiff.head_commit !== ""}
          @ {branchDiff.head_commit.slice(0, 8)}
        {/if}
      </span>
    {/if}
  </div>

  <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-1">
    {#if scope === "session"}
      {#if sessionFiles.length === 0}
        <p class="text-muted-foreground p-2 text-[12px]">{m.diff_session_empty()}</p>
      {:else}
        {#each sessionFiles as file (file.path)}
          {@const absolute = sessionAbsolute(file)}
          {@const badge = statusByPath.get(absolute)}
          <button
            type="button"
            class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
            title={file.path}
            onclick={() => openDiff(absolute)}
          >
            {#if badge !== undefined}
              <GitStatusBadge status={badge} />
            {:else}
              <span class="size-4 shrink-0"></span>
            {/if}
            <span class="min-w-0 flex-1 truncate font-mono">{file.path}</span>
            <span class="text-ok shrink-0 text-[11px]">+{file.additions}</span>
            <span class="text-danger shrink-0 text-[11px]">-{file.deletions}</span>
          </button>
        {/each}
      {/if}
    {:else if scope === "uncommitted"}
      {#if statusLoading && status === null}
        <div class="text-muted-foreground flex items-center justify-center gap-2 p-4 text-[12px]">
          <LoaderCircleIcon class="size-3.5 animate-spin" />
          {m.dock_changes_loading()}
        </div>
      {:else if statusError !== null}
        <p class="text-destructive p-2 text-[12px]">{statusError}</p>
      {:else if status === null}
        <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_loading()}</p>
      {:else if !status.is_repository}
        <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_not_repository()}</p>
      {:else if status.entries.length === 0}
        <p class="text-muted-foreground p-2 text-[12px]">{m.diff_no_changes()}</p>
      {:else}
        {#each status.entries as entry (entry.absolute_path)}
          <button
            type="button"
            class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
            title={entry.old_path === null ? entry.path : `${entry.old_path} → ${entry.path}`}
            onclick={() => openDiff(entry.absolute_path)}
          >
            <GitStatusBadge status={entry.status} />
            <span class="min-w-0 flex-1 truncate font-mono">{entry.path}</span>
          </button>
        {/each}
        {#if status.truncated}
          <p class="text-muted-foreground px-1.5 py-1 text-[11px]">{m.dock_files_truncated()}</p>
        {/if}
      {/if}
    {:else if branchLoading && branchDiff === null}
      <div class="text-muted-foreground flex items-center justify-center gap-2 p-4 text-[12px]">
        <LoaderCircleIcon class="size-3.5 animate-spin" />
        {m.dock_changes_loading()}
      </div>
    {:else if branchError !== null}
      <p class="text-destructive p-2 text-[12px]">{branchError}</p>
    {:else if branchDiff === null}
      <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_loading()}</p>
    {:else if branchDiff.files.length === 0}
      <p class="text-muted-foreground p-2 text-[12px]">{m.diff_no_changes()}</p>
    {:else}
      {#each branchDiff.files as file (file.path)}
        {@const absolute = branchAbsolute(file)}
        <button
          type="button"
          class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
          title={file.path}
          onclick={() => openDiff(absolute, "branch_base")}
        >
          <span class="min-w-0 flex-1 truncate font-mono">{file.path}</span>
          <span class="text-ok shrink-0 text-[11px]">+{file.additions}</span>
          <span class="text-danger shrink-0 text-[11px]">-{file.deletions}</span>
        </button>
      {/each}
    {/if}
  </div>
{/if}
