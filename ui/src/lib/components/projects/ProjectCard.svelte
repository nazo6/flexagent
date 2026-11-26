<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { ProjectSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { ResourceEntry } from "#lib/stores/resource.svelte.ts";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import EraserIcon from "@lucide/svelte/icons/eraser";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import LinkIcon from "@lucide/svelte/icons/link";
  import PlayIcon from "@lucide/svelte/icons/play";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  interface Props {
    project: ProjectSummary;
    worktreeState: ResourceEntry<WorktreeInfo[]>;
    nodeName: (nodeId: string) => string;
    onRefreshWorktrees: (projectId: string) => void;
    onCreateWorktree: (projectId: string) => void;
    onPrune: (projectId: string) => void;
    onNewSession: (projectId: string, nodeId?: string, path?: string) => void;
    onRemoveWorktree: (projectId: string, worktree: WorktreeInfo) => void;
    onUnlinkBinding: (projectId: string, nodeId: string, path: string) => void;
  }

  let {
    project,
    worktreeState,
    nodeName,
    onRefreshWorktrees,
    onCreateWorktree,
    onPrune,
    onNewSession,
    onRemoveWorktree,
    onUnlinkBinding,
  }: Props = $props();

  /** Binding awaiting removal confirmation. */
  let unlinkTarget = $state<{ nodeId: string; path: string } | null>(null);

  const worktrees = $derived(worktreeState.data ?? []);
  const isLocal = $derived(project.identity.kind === "local");
  const identityLabel = $derived(
    project.identity.kind === "git_remote"
      ? m.projects_identity_git_remote()
      : project.identity.kind === "user_key"
        ? m.projects_identity_user_key()
        : project.identity.kind === "local"
          ? m.projects_identity_local()
          : m.projects_identity_unknown(),
  );
</script>

<section class="bg-card border-line-strong flex flex-col gap-3 rounded-lg border p-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div class="flex min-w-0 flex-col gap-1">
      <div class="flex flex-wrap items-center gap-2">
        <h2 class="truncate text-sm font-semibold tracking-tight">{project.name}</h2>
        {#if isLocal}
          <Badge
            class="border-warn/35 bg-warn-soft text-warn gap-1"
            title={m.projects_local_only_hint()}
          >
            <AlertTriangleIcon class="size-3" />
            {identityLabel}
          </Badge>
        {:else}
          <Badge variant="outline">{identityLabel}</Badge>
        {/if}
      </div>
      {#if project.canonical_git_url}
        <p class="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs">
          <LinkIcon class="size-3.5 shrink-0" />
          <span class="truncate">{project.canonical_git_url}</span>
        </p>
      {:else}
        <p
          class="text-muted-foreground desk-mono min-w-0 truncate text-xs"
          title={project.project_id}
        >
          {project.project_id}
        </p>
      {/if}
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button
        variant="outline"
        size="sm"
        disabled={worktreeState.loading}
        onclick={() => onRefreshWorktrees(project.project_id)}
      >
        <RefreshCwIcon class="size-3.5" />
        {m.projects_worktrees()}
      </Button>
      <Button variant="outline" size="sm" onclick={() => onCreateWorktree(project.project_id)}>
        <PlusIcon class="size-3.5" />
        {m.projects_worktree_add()}
      </Button>
      <Button
        variant="outline"
        size="sm"
        onclick={() => onPrune(project.project_id)}
        title={m.projects_prune_title()}
      >
        <EraserIcon class="size-3.5" />
        Prune
      </Button>
      <Button
        size="sm"
        onclick={() => {
          onNewSession(project.project_id);
        }}
      >
        <PlayIcon class="size-3.5" />
        {m.projects_new_session()}
      </Button>
    </div>
  </div>

  {#if project.bindings.length > 0}
    <div class="flex flex-col gap-1.5">
      <h3 class="desk-label">{m.projects_folders()}</h3>
      <ul class="m-0 flex list-none flex-col gap-1 p-0">
        {#each project.bindings as binding (`${binding.node_id}|${binding.local_path}`)}
          <li
            class="hover:border-border flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5 text-xs {binding.path_exists
              ? 'bg-background border-transparent'
              : 'border-danger/20 bg-danger-soft/50'}"
          >
            <Badge variant="secondary">{nodeName(binding.node_id)}</Badge>
            <span
              class="desk-mono min-w-0 grow basis-32 truncate {binding.path_exists
                ? ''
                : 'text-muted-foreground line-through'}"
              title={binding.path_exists
                ? binding.local_path
                : m.projects_path_missing_title({ path: binding.local_path })}
            >
              {binding.local_path}
            </span>
            {#if !binding.path_exists}
              <Badge class="border-danger/35 bg-danger-soft text-danger gap-1">
                <AlertTriangleIcon class="size-3" />
                {m.projects_path_missing()}
              </Badge>
            {/if}
            {#if binding.git_branch}
              <span class="text-muted-foreground">({binding.git_branch})</span>
            {/if}
            {#if binding.is_worktree}
              <Badge variant="outline">worktree</Badge>
            {/if}
            <span class="text-muted-foreground ml-auto"
              >{formatRelativeTime(binding.last_used_at)}</span
            >
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:text-foreground"
              onclick={() => onNewSession(project.project_id, binding.node_id, binding.local_path)}
              title={m.projects_new_session_here()}
            >
              <PlayIcon />
              <span class="sr-only">{m.projects_new_session_here()}</span>
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              class="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
              onclick={() => (unlinkTarget = { nodeId: binding.node_id, path: binding.local_path })}
              title={m.projects_unlink_tooltip()}
            >
              <Trash2Icon />
              <span class="sr-only">{m.projects_unlink_tooltip()}</span>
            </Button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if worktreeState.loading || worktreeState.error !== null || worktrees.length > 0}
    <div class="flex flex-col gap-1.5">
      <h3 class="desk-label">{m.projects_worktrees()}</h3>
      {#if worktreeState.loading && worktrees.length === 0}
        <p class="text-muted-foreground text-xs">{m.common_loading()}</p>
      {:else if worktreeState.error !== null && worktrees.length === 0}
        <div class="flex items-center gap-2 text-xs">
          <p class="text-danger">
            {m.common_load_failed()}
            {worktreeState.error}
          </p>
          <Button variant="ghost" size="xs" onclick={() => onRefreshWorktrees(project.project_id)}>
            {m.common_retry()}
          </Button>
        </div>
      {:else}
        <ul class="m-0 flex list-none flex-col gap-1 p-0">
          {#each worktrees as worktree (`${worktree.node_id}|${worktree.path}`)}
            <li
              class="bg-background hover:border-border flex flex-wrap items-center gap-2 rounded-md border border-transparent px-2 py-1.5 text-xs"
            >
              <GitBranchIcon class="text-muted-foreground size-3.5 shrink-0" />
              <span class="desk-mono shrink-0 font-medium">{worktree.branch ?? "(detached)"}</span>
              <span class="text-muted-foreground min-w-0 grow basis-32 truncate"
                >{worktree.path}</span
              >
              <Badge variant="secondary">{nodeName(worktree.node_id)}</Badge>
              {#if worktree.is_main}
                <Badge variant="secondary">main</Badge>
              {/if}
              {#if worktree.head_commit}
                <span class="text-muted-foreground desk-mono ml-auto">
                  {worktree.head_commit.slice(0, 8)}
                </span>
              {/if}
              {#if !worktree.is_main}
                <Button
                  variant="ghost"
                  size="icon-xs"
                  class="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
                  onclick={() => onRemoveWorktree(project.project_id, worktree)}
                >
                  <Trash2Icon />
                  <span class="sr-only">{m.projects_worktree_delete()}</span>
                </Button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</section>

<AlertDialog.Root
  open={unlinkTarget !== null}
  onOpenChange={(open) => !open && (unlinkTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.projects_unlink_title()}</AlertDialog.Title>
      <AlertDialog.Description>
        {m.projects_unlink_description({
          path: unlinkTarget?.path ?? "",
          node: nodeName(unlinkTarget?.nodeId ?? ""),
        })}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => {
          if (unlinkTarget !== null) {
            onUnlinkBinding(project.project_id, unlinkTarget.nodeId, unlinkTarget.path);
          }
          unlinkTarget = null;
        }}
      >
        {m.projects_unlink_action()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
