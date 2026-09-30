<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import type { ProjectSummary } from '$lib/generated/ProjectSummary';
  import type { WorktreeInfo } from '$lib/generated/WorktreeInfo';
  import { sync } from '$lib/stores/app.svelte';
  import { goto } from '$app/navigation';
  import { toast } from 'svelte-sonner';

  let {
    open = $bindable(false),
    project,
    worktrees
  }: {
    open?: boolean;
    project: ProjectSummary;
    worktrees: WorktreeInfo[];
  } = $props();

  type PathMode = 'existing' | 'new_worktree';

  let nodeId = $state('');
  let agentId = $state('');
  let customAgent = $state('');
  let pathMode = $state<PathMode>('existing');
  let localPath = $state('');
  let branch = $state('');
  let baseBranch = $state('');
  let newPath = $state('');
  let initialPrompt = $state('');
  let busy = $state(false);

  const nodes = $derived(sync.nodes);
  const selectedNode = $derived(nodes.find((node) => node.node_id === nodeId) ?? null);

  const nodePaths = $derived.by(() => {
    const options: { path: string; label: string }[] = [];
    for (const worktree of worktrees) {
      if (worktree.node_id !== nodeId) continue;
      const kind = worktree.is_main ? 'メインリポジトリ' : 'Worktree';
      options.push({
        path: worktree.path,
        label: `${worktree.branch ?? '(detached)'} — ${worktree.path} [${kind}]`
      });
    }
    for (const binding of project.bindings) {
      if (binding.node_id !== nodeId) continue;
      if (options.some((option) => option.path === binding.local_path)) continue;
      options.push({
        path: binding.local_path,
        label: `${binding.git_branch ?? '(unknown)'} — ${binding.local_path}`
      });
    }
    return options;
  });

  const installedAgents = $derived(selectedNode?.installed_agents ?? []);

  $effect(() => {
    if (open) {
      // ダイアログを開くたびに既定値を初期化する
      const firstNode = sync.nodes[0];
      nodeId = firstNode?.node_id ?? '';
      agentId = '';
      customAgent = '';
      pathMode = 'existing';
      localPath = '';
      branch = '';
      baseBranch = '';
      newPath = '';
      initialPrompt = '';
    }
  });

  $effect(() => {
    // ノード切替時にパス選択を先頭へ寄せる
    if (nodePaths.length > 0 && !nodePaths.some((option) => option.path === localPath)) {
      localPath = nodePaths[0].path;
    }
  });

  const effectiveAgent = $derived(
    installedAgents.length > 0 ? (agentId || installedAgents[0]) : customAgent.trim()
  );

  const canSubmit = $derived(
    !busy &&
      nodeId !== '' &&
      effectiveAgent !== '' &&
      (pathMode === 'existing' ? localPath !== '' : branch.trim() !== '')
  );

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!canSubmit) return;
    busy = true;
    try {
      const response = await sync.connection.client.createSession({
        command_id: crypto.randomUUID(),
        project_id: project.project_id,
        node_id: nodeId,
        provisioner: null,
        local_path: pathMode === 'existing' ? localPath : null,
        worktree:
          pathMode === 'new_worktree'
            ? {
                branch: branch.trim(),
                base_branch: baseBranch.trim() === '' ? null : baseBranch.trim(),
                new_path: newPath.trim() === '' ? null : newPath.trim()
              }
            : null,
        agent_id: effectiveAgent,
        initial_prompt: initialPrompt.trim() === '' ? null : initialPrompt,
        fork: null
      });
      toast.success('セッションを開始しました');
      open = false;
      await sync.refreshSessions();
      await goto(`/sessions/${response.session_id}`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>新規セッション</Dialog.Title>
      <Dialog.Description>
        {project.name} (プロジェクト {project.project_id})
      </Dialog.Description>
    </Dialog.Header>

    <form class="flex flex-col gap-4" onsubmit={submit}>
      <div class="grid gap-2">
        <Label for="session-node">実行ノード</Label>
        <select
          id="session-node"
          bind:value={nodeId}
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
        >
          {#each nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name} ({node.os}/{node.arch}){node.is_online ? '' : ' — オフライン'}
            </option>
          {/each}
        </select>
      </div>

      <div class="grid gap-2">
        <Label for="session-agent">エージェント</Label>
        {#if installedAgents.length > 0}
          <select
            id="session-agent"
            bind:value={agentId}
            class="border-input bg-background h-9 rounded-md border px-2 text-sm"
          >
            {#each installedAgents as agent (agent)}
              <option value={agent}>{agent}</option>
            {/each}
          </select>
        {:else}
          <Input
            id="session-agent"
            bind:value={customAgent}
            placeholder="エージェントID (例: opencode2)"
          />
        {/if}
      </div>

      <fieldset class="flex flex-col gap-2">
        <legend class="text-sm font-medium">実行ディレクトリ</legend>
        <div class="flex gap-4 text-sm">
          <label class="flex items-center gap-1.5">
            <input type="radio" bind:group={pathMode} value="existing" />
            既存の Worktree / リポジトリ
          </label>
          <label class="flex items-center gap-1.5">
            <input type="radio" bind:group={pathMode} value="new_worktree" />
            新規 Worktree を作成
          </label>
        </div>

        {#if pathMode === 'existing'}
          {#if nodePaths.length > 0}
            <select
              bind:value={localPath}
              class="border-input bg-background h-9 rounded-md border px-2 text-sm"
            >
              {#each nodePaths as option (option.path)}
                <option value={option.path}>{option.label}</option>
              {/each}
            </select>
          {:else}
            <Input bind:value={localPath} placeholder="絶対パスを入力 (例: C:\\repos\\app)" />
            <p class="text-muted-foreground text-xs">
              このノードには登録済みパスがありません。パスを直接入力してください。
            </p>
          {/if}
        {:else}
          <div class="grid gap-2 sm:grid-cols-2">
            <div class="grid gap-1">
              <Label for="session-branch">ブランチ名</Label>
              <Input id="session-branch" bind:value={branch} placeholder="feat/auth" />
            </div>
            <div class="grid gap-1">
              <Label for="session-base">起点ブランチ (任意)</Label>
              <Input id="session-base" bind:value={baseBranch} placeholder="main" />
            </div>
          </div>
          <div class="grid gap-1">
            <Label for="session-new-path">配置先パス (任意)</Label>
            <Input id="session-new-path" bind:value={newPath} placeholder="自動解決" />
          </div>
        {/if}
      </fieldset>

      <div class="grid gap-2">
        <Label for="session-prompt">初期プロンプト (任意)</Label>
        <textarea
          id="session-prompt"
          bind:value={initialPrompt}
          rows="2"
          class="border-input bg-background w-full rounded-md border px-3 py-2 text-sm"
          placeholder="例: このリポジトリの構成を説明して"
        ></textarea>
      </div>

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (open = false)}>
          キャンセル
        </Button>
        <Button type="submit" disabled={!canSubmit}>
          {busy ? '起動中…' : 'セッションを開始'}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
