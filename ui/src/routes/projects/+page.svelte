<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import * as Dialog from '$lib/components/ui/dialog';
  import NewSessionDialog from '$lib/components/NewSessionDialog.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { formatRelativeTime } from '$lib/format';
  import type { ProjectSummary } from '$lib/generated/ProjectSummary';
  import type { WorktreeInfo } from '$lib/generated/WorktreeInfo';
  import { shortId } from '$lib/session-status';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import GitBranchIcon from '@lucide/svelte/icons/git-branch';
  import PlayIcon from '@lucide/svelte/icons/play';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  let worktreesByProject = $state(new Map<string, WorktreeInfo[]>());
  let loadingProjects = $state<string[]>([]);

  // 新規 Worktree 作成ダイアログ
  let createProjectId = $state<string | null>(null);
  let createNodeId = $state('');
  let createBranch = $state('');
  let createBase = $state('');
  let createPath = $state('');
  let createBusy = $state(false);

  // Worktree 削除確認
  let removeTarget = $state<{ projectId: string; worktree: WorktreeInfo } | null>(null);
  let removeBusy = $state(false);

  // 新規セッションダイアログ
  let sessionProject = $state<ProjectSummary | null>(null);
  let sessionOpen = $state(false);

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }

  async function loadWorktrees(projectId: string, silent = false): Promise<void> {
    if (loadingProjects.includes(projectId)) return;
    loadingProjects = [...loadingProjects, projectId];
    try {
      const list = await sync.connection.client.worktrees(projectId);
      worktreesByProject = new Map(worktreesByProject).set(projectId, list);
    } catch (error) {
      if (!silent) toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loadingProjects = loadingProjects.filter((id) => id !== projectId);
    }
  }

  function openCreateWorktree(projectId: string) {
    createProjectId = projectId;
    createNodeId = sync.nodes.find((node) => node.is_online)?.node_id ?? '';
    createBranch = '';
    createBase = '';
    createPath = '';
  }

  async function submitCreateWorktree(event: SubmitEvent) {
    event.preventDefault();
    const projectId = createProjectId;
    if (projectId === null || createBusy) return;
    createBusy = true;
    try {
      const created = await sync.connection.client.createWorktree(projectId, {
        node_id: createNodeId,
        branch: createBranch.trim(),
        base_branch: createBase.trim() === '' ? null : createBase.trim(),
        path: createPath.trim() === '' ? null : createPath.trim()
      });
      toast.success(`Worktree を作成しました: ${created.path}`);
      createProjectId = null;
      await loadWorktrees(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      createBusy = false;
    }
  }

  async function confirmRemove(force: boolean) {
    const target = removeTarget;
    if (target === null || removeBusy) return;
    removeBusy = true;
    try {
      await sync.connection.client.removeWorktree(target.projectId, {
        node_id: target.worktree.node_id,
        path: target.worktree.path,
        force
      });
      toast.success('Worktree を削除しました');
      removeTarget = null;
      await loadWorktrees(target.projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      removeBusy = false;
    }
  }
</script>

<div class="flex flex-col gap-4">
  <div class="flex items-center justify-between gap-2">
    <div>
      <h1 class="text-xl font-semibold">プロジェクト</h1>
      <p class="text-muted-foreground text-sm">
        論理プロジェクトごとのノード紐付けと Git Worktree
      </p>
    </div>
    <Button variant="outline" size="sm" onclick={() => void sync.refreshProjects()}>
      <RefreshCwIcon />
      再読込
    </Button>
  </div>

  {#if sync.projects.length === 0}
    <p class="text-muted-foreground text-sm">登録済みのプロジェクトがありません。</p>
  {:else}
    {#each sync.projects as project (project.project_id)}
      <Card class="gap-3 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="flex min-w-0 flex-col gap-0.5">
            <h2 class="truncate font-medium">{project.name}</h2>
            <p class="text-muted-foreground truncate text-xs">{project.project_id}</p>
            {#if project.canonical_git_url}
              <p class="text-muted-foreground truncate text-xs">{project.canonical_git_url}</p>
            {/if}
          </div>
          <div class="flex flex-wrap items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              disabled={loadingProjects.includes(project.project_id)}
              onclick={() => void loadWorktrees(project.project_id)}
            >
              <RefreshCwIcon />
              Worktree
            </Button>
            <Button
              variant="outline"
              size="sm"
              onclick={() => openCreateWorktree(project.project_id)}
            >
              <PlusIcon />
              Worktree 追加
            </Button>
            <Button
              size="sm"
              onclick={() => {
                sessionProject = project;
                sessionOpen = true;
                void loadWorktrees(project.project_id, true);
              }}
            >
              <PlayIcon />
              新規セッション
            </Button>
          </div>
        </div>

        {#if project.bindings.length > 0}
          <ul class="flex list-none flex-col gap-1 p-0 text-xs">
            {#each project.bindings as binding (`${binding.node_id}|${binding.local_path}`)}
              <li class="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{nodeName(binding.node_id)}</Badge>
                <span class="font-mono">{binding.local_path}</span>
                {#if binding.git_branch}
                  <span class="text-muted-foreground">({binding.git_branch})</span>
                {/if}
                {#if binding.is_worktree}
                  <Badge variant="secondary">worktree</Badge>
                {/if}
                <span class="text-muted-foreground ml-auto">
                  {formatRelativeTime(binding.last_used_at)}
                </span>
              </li>
            {/each}
          </ul>
        {/if}

        {@const worktrees = worktreesByProject.get(project.project_id) ?? []}
        {#if worktrees.length > 0}
          <ul class="flex list-none flex-col gap-1 p-0 text-xs">
            {#each worktrees as worktree (`${worktree.node_id}|${worktree.path}`)}
              <li class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md px-2 py-1.5">
                <GitBranchIcon class="text-muted-foreground size-3.5" />
                <span class="font-mono">{worktree.branch ?? '(detached)'}</span>
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
                    onclick={() => (removeTarget = { projectId: project.project_id, worktree })}
                  >
                    <Trash2Icon />
                    <span class="sr-only">Worktree を削除</span>
                  </Button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </Card>
    {/each}
  {/if}
</div>

<!-- 新規 Worktree 作成 -->
<Dialog.Root
  open={createProjectId !== null}
  onOpenChange={(open) => !open && (createProjectId = null)}
>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Worktree を作成</Dialog.Title>
      <Dialog.Description>
        <code>git worktree add -b &lt;branch&gt;</code> を実行します。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitCreateWorktree}>
      <div class="grid gap-2">
        <Label for="wt-node">ノード</Label>
        <select
          id="wt-node"
          bind:value={createNodeId}
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
        >
          {#each sync.nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}{node.is_online ? '' : ' — オフライン'}
            </option>
          {/each}
        </select>
      </div>
      <div class="grid gap-2">
        <Label for="wt-branch">ブランチ名</Label>
        <Input id="wt-branch" bind:value={createBranch} placeholder="feat/new-feature" />
      </div>
      <div class="grid gap-2">
        <Label for="wt-base">起点ブランチ (任意)</Label>
        <Input id="wt-base" bind:value={createBase} placeholder="main" />
      </div>
      <div class="grid gap-2">
        <Label for="wt-path">配置先パス (任意)</Label>
        <Input id="wt-path" bind:value={createPath} placeholder="自動解決" />
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (createProjectId = null)}>
          キャンセル
        </Button>
        <Button type="submit" disabled={createBusy || createBranch.trim() === ''}>
          {createBusy ? '作成中…' : '作成'}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- Worktree 削除確認 -->
<AlertDialog.Root
  open={removeTarget !== null}
  onOpenChange={(open) => !open && (removeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Worktree を削除しますか？</AlertDialog.Title>
      <AlertDialog.Description>
        {removeTarget?.worktree.path} ({nodeName(removeTarget?.worktree.node_id ?? '')})
        を削除します。未コミットの変更がある場合、強制削除で失われます。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={removeBusy}
        onclick={() => void confirmRemove(false)}
      >
        削除
      </AlertDialog.Action>
      <AlertDialog.Action
        variant="destructive"
        disabled={removeBusy}
        onclick={() => void confirmRemove(true)}
      >
        強制削除
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

{#if sessionProject !== null}
  <NewSessionDialog
    bind:open={sessionOpen}
    project={sessionProject}
    worktrees={worktreesByProject.get(sessionProject.project_id) ?? []}
  />
{/if}
