<script lang="ts">
  import { goto } from '$app/navigation';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import * as Dialog from '$lib/components/ui/dialog';
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
  import FolderBrowserDialog from '$lib/components/fs/FolderBrowserDialog.svelte';
  import EraserIcon from '@lucide/svelte/icons/eraser';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import GitBranchIcon from '@lucide/svelte/icons/git-branch';
  import LinkIcon from '@lucide/svelte/icons/link';
  import PlayIcon from '@lucide/svelte/icons/play';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import SearchIcon from '@lucide/svelte/icons/search';
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
  let browserOpen = $state(false);
  /** フォルダブラウザの選択結果の反映先。 */
  let browserTarget = $state<'create' | 'scan' | 'link'>('create');

  // Worktree 削除確認
  let removeTarget = $state<{ projectId: string; worktree: WorktreeInfo } | null>(null);
  let removeBusy = $state(false);

  // Worktree クリーンアップ (Prune) 確認
  let pruneProjectId = $state<string | null>(null);
  let pruneNodeId = $state('');
  let pruneBusy = $state(false);

  // プロジェクト一括スキャン (Scan)
  let scanOpen = $state(false);
  let scanNodeId = $state('');
  let scanDir = $state('');
  let scanBusy = $state(false);

  // フォルダ手動紐付け (Link)
  let linkOpen = $state(false);
  let linkNodeId = $state('');
  let linkProjectId = $state('');
  let linkPath = $state('');
  let linkBusy = $state(false);

  /** フォルダブラウザに渡す閲覧先ノード。 */
  const browserNodeId = $derived(
    browserTarget === 'create' ? createNodeId : browserTarget === 'scan' ? scanNodeId : linkNodeId
  );

  /** フォルダブラウザの現在パス。 */
  const browserInitialPath = $derived(
    browserTarget === 'create' ? createPath : browserTarget === 'scan' ? scanDir : linkPath
  );

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

  /** このプロジェクトの新規セッション画面を開く (場所・ノードを指定)。 */
  function openNewSession(projectId: string, bindingNodeId?: string, path?: string) {
    const params = new URLSearchParams({ project: projectId });
    if (bindingNodeId) params.set('node', bindingNodeId);
    if (path) params.set('path', path);
    void goto(`/?${params.toString()}`);
  }

  /** オンラインノードの既定選択 (先頭)。 */
  function defaultOnlineNode(): string {
    return sync.nodes.find((node) => node.is_online)?.node_id ?? '';
  }

  function openScanDialog() {
    scanNodeId = defaultOnlineNode();
    scanDir = '';
    scanOpen = true;
  }

  async function submitScan(event: SubmitEvent) {
    event.preventDefault();
    if (scanBusy || scanNodeId === '') return;
    scanBusy = true;
    try {
      const response = await sync.connection.client.scanProjects(scanNodeId, {
        dir: scanDir.trim() === '' ? null : scanDir.trim()
      });
      toast.success(
        `スキャン完了: ${response.scanned_dirs.length} ディレクトリを走査し、${response.projects.length} 件のプロジェクトが登録されています`
      );
      scanOpen = false;
      await sync.refreshProjects();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      scanBusy = false;
    }
  }

  function openLinkDialog() {
    linkNodeId = defaultOnlineNode();
    linkProjectId = '';
    linkPath = '';
    linkOpen = true;
  }

  async function submitLink(event: SubmitEvent) {
    event.preventDefault();
    if (linkBusy || linkNodeId === '' || linkProjectId.trim() === '' || linkPath.trim() === '') {
      return;
    }
    linkBusy = true;
    try {
      const result = await sync.connection.client.linkProject(linkNodeId, {
        project_id: linkProjectId.trim(),
        local_path: linkPath.trim()
      });
      toast.success(`紐付けました: ${result.local_path} → ${result.project_id}`);
      linkOpen = false;
      await sync.refreshProjects();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      linkBusy = false;
    }
  }

  /** このプロジェクトに紐付けのあるオンラインノードを優先して Prune 対象に選ぶ。 */
  function openPruneDialog(projectId: string) {
    pruneProjectId = projectId;
    const project = sync.projects.find((entry) => entry.project_id === projectId);
    const boundOnline = project?.bindings.find((binding) =>
      sync.nodes.some((node) => node.node_id === binding.node_id && node.is_online)
    );
    pruneNodeId = boundOnline?.node_id ?? defaultOnlineNode();
  }

  async function confirmPrune() {
    const projectId = pruneProjectId;
    if (projectId === null || pruneBusy || pruneNodeId === '') return;
    pruneBusy = true;
    try {
      await sync.connection.client.pruneWorktrees(projectId, { node_id: pruneNodeId });
      toast.success('Worktree 管理情報をクリーンアップしました');
      pruneProjectId = null;
      await loadWorktrees(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pruneBusy = false;
    }
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

<div class="mx-auto flex max-w-5xl flex-col gap-4 p-4 md:p-6">
  <div class="flex items-center justify-between gap-2">
    <div>
      <h1 class="text-xl font-semibold">プロジェクト</h1>
      <p class="text-muted-foreground text-sm">
        論理プロジェクトごとのノード紐付けと Git Worktree
      </p>
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button variant="outline" size="sm" onclick={openScanDialog} title="ノード上の Git リポジトリを一括スキャンして登録">
        <SearchIcon />
        スキャン
      </Button>
      <Button variant="outline" size="sm" onclick={openLinkDialog} title="任意のフォルダを論理プロジェクトへ手動紐付け">
        <LinkIcon />
        フォルダを紐付け
      </Button>
      <Button variant="outline" size="sm" onclick={() => void sync.refreshProjects()}>
        <RefreshCwIcon />
        再読込
      </Button>
    </div>
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
              variant="outline"
              size="sm"
              onclick={() => openPruneDialog(project.project_id)}
              title="削除済み Worktree の管理情報をクリーンアップ (git worktree prune)"
            >
              <EraserIcon />
              Prune
            </Button>
            <Button
              size="sm"
              onclick={() => {
                openNewSession(project.project_id);
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
                <span
                  class="font-mono {binding.path_exists ? '' : 'text-muted-foreground line-through'}"
                  title={binding.path_exists
                    ? binding.local_path
                    : `${binding.local_path} (ノード上に存在しません)`}
                >
                  {binding.local_path}
                </span>
                {#if !binding.path_exists}
                  <Badge variant="outline" class="border-destructive/50 text-destructive">
                    存在しません
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
                  onclick={() => openNewSession(project.project_id, binding.node_id, binding.local_path)}
                  title="この場所で新規セッションを開始"
                >
                  <PlayIcon class="size-3.5" />
                  <span class="sr-only">この場所で新規セッション</span>
                </Button>
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
        <div class="flex items-center gap-1.5">
          <Input id="wt-path" bind:value={createPath} placeholder="自動解決" class="flex-1 font-mono text-xs" />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!createNodeId}
            onclick={() => {
              browserTarget = 'create';
              browserOpen = true;
            }}
            title="フォルダブラウザで選択"
          >
            <FolderOpenIcon class="size-3.5" />
            <span>参照…</span>
          </Button>
        </div>
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

<!-- Worktree クリーンアップ (Prune) 確認 -->
<AlertDialog.Root
  open={pruneProjectId !== null}
  onOpenChange={(open) => !open && (pruneProjectId = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title class="flex items-center gap-2">
        <EraserIcon class="size-5" />
        Worktree 管理情報をクリーンアップしますか？
      </AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        <code>git worktree prune</code>
        を実行し、削除済みディレクトリの管理情報を削除します。作業中の Worktree やファイルは削除されません。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <div class="grid gap-2">
      <Label for="prune-node">実行ノード</Label>
      <select
        id="prune-node"
        bind:value={pruneNodeId}
        class="border-input bg-background h-9 rounded-md border px-2 text-sm"
      >
        {#each sync.nodes as node (node.node_id)}
          <option value={node.node_id} disabled={!node.is_online}>
            {node.name}{node.is_online ? '' : ' — オフライン'}
          </option>
        {/each}
      </select>
    </div>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={pruneBusy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action disabled={pruneBusy || pruneNodeId === ''} onclick={() => void confirmPrune()}>
        {pruneBusy ? '実行中…' : 'Prune 実行'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- プロジェクト一括スキャン (Scan) -->
<Dialog.Root open={scanOpen} onOpenChange={(open) => (scanOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>リポジトリをスキャン</Dialog.Title>
      <Dialog.Description>
        指定ディレクトリ配下の Git リポジトリを探索し、論理プロジェクトとして登録します。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitScan}>
      <div class="grid gap-2">
        <Label for="scan-node">ノード</Label>
        <select
          id="scan-node"
          bind:value={scanNodeId}
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
        <Label for="scan-dir">対象ディレクトリ (任意)</Label>
        <div class="flex items-center gap-1.5">
          <Input
            id="scan-dir"
            bind:value={scanDir}
            placeholder="未指定時は config.toml の project_scan_dirs"
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!scanNodeId}
            onclick={() => {
              browserTarget = 'scan';
              browserOpen = true;
            }}
            title="フォルダブラウザで選択"
          >
            <FolderOpenIcon class="size-3.5" />
            <span>参照…</span>
          </Button>
        </div>
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (scanOpen = false)}>
          キャンセル
        </Button>
        <Button type="submit" disabled={scanBusy || scanNodeId === ''}>
          {scanBusy ? 'スキャン中…' : 'スキャン'}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- フォルダ手動紐付け (Link) -->
<Dialog.Root open={linkOpen} onOpenChange={(open) => (linkOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>フォルダを紐付け</Dialog.Title>
      <Dialog.Description>
        任意のディレクトリを論理プロジェクト ID へ手動で紐付けます (`.fxg.toml` に
        <code>project_key</code> を書き込みます)。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitLink}>
      <div class="grid gap-2">
        <Label for="link-node">ノード</Label>
        <select
          id="link-node"
          bind:value={linkNodeId}
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
        <Label for="link-project">論理プロジェクト ID</Label>
        <Input
          id="link-project"
          bind:value={linkProjectId}
          placeholder="github.com/nazo6/flexagent"
          class="font-mono text-xs"
        />
      </div>
      <div class="grid gap-2">
        <Label for="link-path">対象ディレクトリ</Label>
        <div class="flex items-center gap-1.5">
          <Input
            id="link-path"
            bind:value={linkPath}
            placeholder="D:\\ghq\\github.com\\nazo6\\flexagent"
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!linkNodeId}
            onclick={() => {
              browserTarget = 'link';
              browserOpen = true;
            }}
            title="フォルダブラウザで選択"
          >
            <FolderOpenIcon class="size-3.5" />
            <span>参照…</span>
          </Button>
        </div>
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (linkOpen = false)}>
          キャンセル
        </Button>
        <Button
          type="submit"
          disabled={linkBusy || linkNodeId === '' || linkProjectId.trim() === '' || linkPath.trim() === ''}
        >
          {linkBusy ? '紐付け中…' : '紐付け'}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<FolderBrowserDialog
  bind:open={browserOpen}
  nodeId={browserNodeId}
  initialPath={browserInitialPath}
  title={browserTarget === 'create'
    ? 'Worktree 配置先フォルダを選択'
    : browserTarget === 'scan'
      ? 'スキャン対象フォルダを選択'
      : '紐付け対象フォルダを選択'}
  onSelect={(path) => {
    if (browserTarget === 'create') createPath = path;
    else if (browserTarget === 'scan') scanDir = path;
    else linkPath = path;
  }}
/>
