<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { ProjectSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { ResourceEntry } from "#lib/stores/resource.svelte.ts";
  import EraserIcon from "@lucide/svelte/icons/eraser";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
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
</script>

<section class="border-border flex flex-col gap-3 border-b py-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div class="flex min-w-0 flex-col gap-1">
      <h2 class="truncate text-sm font-semibold tracking-tight">{project.name}</h2>
      <p class="text-muted-foreground desk-mono truncate text-[11px]">
        {project.project_id}
      </p>
      {#if project.canonical_git_url}
        <p class="text-muted-foreground truncate text-[11px]">
          {project.canonical_git_url}
        </p>
      {/if}
      <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
        <Badge variant="outline" class="h-5 text-[11px]">
          {project.identity.kind === "git_remote"
            ? "Git"
            : project.identity.kind === "user_key"
              ? "User key"
              : "Local only"}
        </Badge>
        {#if project.identity.kind === "local"}
          <span class="text-muted-foreground text-[11px]">
            This machine only. Set a project key to share it across machines.
          </span>
        {/if}
      </div>
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button
        variant="outline"
        size="sm"
        class="h-7 text-[11px]"
        disabled={worktreeState.loading}
        onclick={() => onRefreshWorktrees(project.project_id)}
      >
        <RefreshCwIcon class="size-3.5" />
        {m.projects_worktrees()}
      </Button>
      <Button
        variant="outline"
        size="sm"
        class="h-7 text-[11px]"
        onclick={() => onCreateWorktree(project.project_id)}
      >
        <PlusIcon class="size-3.5" />
        {m.projects_worktree_add()}
      </Button>
      <Button
        variant="outline"
        size="sm"
        class="h-7 text-[11px]"
        onclick={() => onPrune(project.project_id)}
        title={m.projects_prune_title()}
      >
        <EraserIcon class="size-3.5" />
        Prune
      </Button>
      <Button
        size="sm"
        class="h-7 text-[11px]"
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
    <ul class="border-border ml-1 flex list-none flex-col gap-1 border-l p-0 pl-3 text-[11px]">
      {#each project.bindings as binding (`${binding.node_id}|${binding.local_path}`)}
        <li class="flex flex-wrap items-center gap-2">
          <Badge variant="outline">{nodeName(binding.node_id)}</Badge>
          <span
            class="font-mono {binding.path_exists ? '' : 'text-muted-foreground line-through'}"
            title={binding.path_exists
              ? binding.local_path
              : m.projects_path_missing_title({ path: binding.local_path })}
          >
            {binding.local_path}
          </span>
          {#if !binding.path_exists}
            <Badge variant="outline" class="border-destructive/50 text-destructive">
              {m.projects_path_missing()}
            </Badge>
          {/if}
          {#if binding.git_branch}
            <span class="text-muted-foreground">({binding.git_branch})</span>
          {/if}
          {#if binding.is_worktree}
            <Badge variant="secondary">worktree</Badge>
          {/if}
          <Button
            variant="ghost"
            size="icon-sm"
            class="text-muted-foreground hover:text-foreground"
            onclick={() => onNewSession(project.project_id, binding.node_id, binding.local_path)}
            title={m.projects_new_session_here()}
          >
            <PlayIcon class="size-3.5" />
            <span class="sr-only">{m.projects_new_session_here()}</span>
          </Button>
          <span class="text-muted-foreground ml-auto">
            {formatRelativeTime(binding.last_used_at)}
          </span>
          <Button
            variant="ghost"
            size="icon-sm"
            class="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
            onclick={() => (unlinkTarget = { nodeId: binding.node_id, path: binding.local_path })}
            title={m.projects_unlink_tooltip()}
          >
            <Trash2Icon class="size-3.5" />
            <span class="sr-only">{m.projects_unlink_tooltip()}</span>
          </Button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if worktreeState.loading && worktrees.length === 0}
    <p class="text-muted-foreground text-[11px]">{m.common_loading()}</p>
  {:else if worktreeState.error !== null && worktrees.length === 0}
    <div class="flex items-center gap-2 text-[11px]">
      <p class="text-destructive">
        {m.common_load_failed()}
        {worktreeState.error}
      </p>
      <Button variant="ghost" size="xs" onclick={() => onRefreshWorktrees(project.project_id)}>
        {m.common_retry()}
      </Button>
    </div>
  {/if}
  {#if worktrees.length > 0}
    <ul class="border-border ml-1 flex list-none flex-col gap-1 border-l p-0 pl-3 text-[11px]">
      {#each worktrees as worktree (`${worktree.node_id}|${worktree.path}`)}
        <li class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md px-2 py-1.5">
          <GitBranchIcon class="text-muted-foreground size-3.5" />
          <span class="font-mono">{worktree.branch ?? "(detached)"}</span>
          <span class="text-muted-foreground truncate">{worktree.path}</span>
          <Badge variant="outline">{nodeName(worktree.node_id)}</Badge>
          {#if worktree.is_main}
            <Badge variant="secondary">main</Badge>
          {/if}
          {#if worktree.head_commit}
            <span class="text-muted-foreground font-mono">
              {worktree.head_commit.slice(0, 8)}
            </span>
          {/if}
          {#if !worktree.is_main}
            <Button
              variant="ghost"
              size="icon-sm"
              class="text-muted-foreground hover:text-destructive ml-auto"
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
