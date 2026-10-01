<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import { getNodeAvailability } from '$lib/node-status';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import BotIcon from '@lucide/svelte/icons/bot';
  import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';
  import FolderGit2Icon from '@lucide/svelte/icons/folder-git-2';
  import PlayIcon from '@lucide/svelte/icons/play';
  import ServerIcon from '@lucide/svelte/icons/server';
  import Settings2Icon from '@lucide/svelte/icons/settings-2';
  import SparklesIcon from '@lucide/svelte/icons/sparkles';

  type PathMode = 'existing' | 'new_worktree';

  let selectedProjectId = $state('');
  let nodeId = $state('');
  let agentId = $state('');
  let customAgent = $state('');
  let pathMode = $state<PathMode>('existing');
  let localPath = $state('');
  let branch = $state('');
  let baseBranch = $state('');
  let newPath = $state('');
  let promptText = $state('');
  let busy = $state(false);
  let showAdvanced = $state(false);

  const projects = $derived(sync.projects);
  const selectedProject = $derived(
    projects.find((p) => p.project_id === selectedProjectId) ?? projects[0] ?? null
  );

  const nodes = $derived(sync.nodes);
  const selectedNode = $derived(nodes.find((n) => n.node_id === nodeId) ?? null);
  const installedAgents = $derived(selectedNode?.installed_agents ?? []);

  $effect(() => {
    const urlProj = page.url.searchParams.get('project');
    if (urlProj && projects.some((p) => p.project_id === urlProj)) {
      selectedProjectId = urlProj;
    } else if (!selectedProjectId && projects.length > 0) {
      selectedProjectId = projects[0].project_id;
    }
  });

  $effect(() => {
    if (!nodeId && nodes.length > 0) {
      const onlineNode = nodes.find((n) => n.is_online);
      nodeId = (onlineNode ?? nodes[0]).node_id;
    }
  });

  const nodePaths = $derived.by(() => {
    if (!selectedProject) return [];
    const options: { path: string; label: string }[] = [];
    for (const binding of selectedProject.bindings) {
      if (binding.node_id !== nodeId) continue;
      options.push({
        path: binding.local_path,
        label: `${binding.git_branch ? `[${binding.git_branch}] ` : ''}${binding.local_path}`
      });
    }
    return options;
  });

  $effect(() => {
    if (nodePaths.length > 0 && !nodePaths.some((opt) => opt.path === localPath)) {
      localPath = nodePaths[0].path;
    }
  });

  const effectiveAgent = $derived(
    installedAgents.length > 0 ? (agentId || installedAgents[0]) : customAgent.trim()
  );

  const canSubmit = $derived(
    !busy &&
      selectedProject !== null &&
      nodeId !== '' &&
      effectiveAgent !== '' &&
      (pathMode === 'existing' ? localPath !== '' : branch.trim() !== '')
  );

  async function handleStartSession(initialPrompt?: string) {
    if (!canSubmit || !selectedProject) return;
    busy = true;
    const promptToSend = (initialPrompt ?? promptText).trim();

    try {
      const response = await sync.connection.client.createSession({
        command_id: crypto.randomUUID(),
        project_id: selectedProject.project_id,
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
        initial_prompt: promptToSend === '' ? null : promptToSend,
        fork: null
      });

      toast.success('セッションを開始しました');
      await sync.refreshSessions();
      await goto(`/sessions/${response.session_id}`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      void handleStartSession();
    }
  }

  const suggestionPrompts = [
    'このリポジトリの構成とアーキテクチャを説明して',
    '最近の変更点や未コミットの差分を確認して',
    'テストを実行して現在のステータスを確認して',
    'コードベース内の TODO や課題を洗い出して'
  ];
</script>

<div class="mx-auto flex h-full max-w-3xl flex-col justify-between px-4 py-6 md:py-10">
  <div class="flex flex-col gap-6">
    <div class="flex flex-col items-center gap-2 text-center">
      <div class="bg-primary/10 text-primary flex size-12 items-center justify-center rounded-2xl">
        <SparklesIcon class="size-6" />
      </div>
      <h1 class="text-2xl font-bold tracking-tight">何を作業しますか？</h1>
      <p class="text-muted-foreground text-sm max-w-md">
        プロジェクトとエージェントを選択してプロンプトを入力すると、新しい作業セッションを開始できます。
      </p>
    </div>

    <div class="bg-card/70 rounded-xl border p-3.5 shadow-xs flex flex-col gap-3">
      <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            <FolderGit2Icon class="size-3.5" />
            プロジェクト
          </Label>
          {#if projects.length > 0}
            <select
              bind:value={selectedProjectId}
              class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
            >
              {#each projects as project (project.project_id)}
                <option value={project.project_id}>{project.name}</option>
              {/each}
            </select>
          {:else}
            <a
              href="/projects"
              class="border-input bg-muted/40 hover:bg-muted text-muted-foreground flex h-8 items-center justify-center rounded-md border text-xs"
            >
              プロジェクトを登録
            </a>
          {/if}
        </div>

        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            <ServerIcon class="size-3.5" />
            実行ノード
          </Label>
          <select
            bind:value={nodeId}
            class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
          >
            {#each nodes as node (node.node_id)}
              {@const avail = getNodeAvailability(node)}
              <option value={node.node_id}>
                {node.name} ({avail.statusText})
              </option>
            {/each}
          </select>
        </div>

        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            <BotIcon class="size-3.5" />
            エージェント
          </Label>
          {#if installedAgents.length > 0}
            <select
              bind:value={agentId}
              class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
            >
              {#each installedAgents as agent (agent)}
                <option value={agent}>{agent}</option>
              {/each}
            </select>
          {:else}
            <Input
              bind:value={customAgent}
              placeholder="opencode2 / acp"
              class="h-8 text-xs"
            />
          {/if}
        </div>
      </div>

      <div>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
          onclick={() => (showAdvanced = !showAdvanced)}
        >
          <Settings2Icon class="size-3" />
          <span>{showAdvanced ? '詳細設定を閉じる' : 'ブランチ / Worktree 設定'}</span>
        </button>

        {#if showAdvanced}
          <div class="border-t pt-3 mt-2 flex flex-col gap-3">
            <div class="flex gap-4 text-xs">
              <label class="flex items-center gap-1.5 cursor-pointer">
                <input type="radio" bind:group={pathMode} value="existing" />
                既存のリポジトリ
              </label>
              <label class="flex items-center gap-1.5 cursor-pointer">
                <input type="radio" bind:group={pathMode} value="new_worktree" />
                新規 Worktree を作成 (独立ブランチ)
              </label>
            </div>

            {#if pathMode === 'existing'}
              {#if nodePaths.length > 0}
                <select
                  bind:value={localPath}
                  class="border-input bg-background h-8 rounded-md border px-2 text-xs"
                >
                  {#each nodePaths as opt (opt.path)}
                    <option value={opt.path}>{opt.label}</option>
                  {/each}
                </select>
              {:else}
                <Input
                  bind:value={localPath}
                  placeholder="絶対パスを入力 (例: /home/user/project or C:\repo)"
                  class="h-8 text-xs"
                />
              {/if}
            {:else}
              <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <div class="flex flex-col gap-1">
                  <Label class="text-xs">新ブランチ名</Label>
                  <Input bind:value={branch} placeholder="feat/new-task" class="h-8 text-xs" />
                </div>
                <div class="flex flex-col gap-1">
                  <Label class="text-xs">起点ブランチ (任意)</Label>
                  <Input bind:value={baseBranch} placeholder="main" class="h-8 text-xs" />
                </div>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>

    <div class="bg-card rounded-2xl border shadow-sm p-3 flex flex-col gap-2 focus-within:border-primary/50 transition-colors">
      <textarea
        bind:value={promptText}
        onkeydown={onKeydown}
        rows="4"
        placeholder="作業内容や指示を入力してください… (例: 〇〇機能のバグを調査して修正案を出して)&#10;Ctrl + Enter でセッションを開始します"
        class="bg-transparent placeholder:text-muted-foreground w-full resize-none text-sm outline-none leading-relaxed"
        disabled={busy}
      ></textarea>

      <div class="flex items-center justify-between border-t pt-2.5">
        <div class="flex items-center gap-2">
          {#if selectedNode}
            <NodeStatusBadge node={selectedNode} showNodeName={true} />
          {/if}
        </div>

        <div class="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            class="text-xs h-8 text-muted-foreground hover:text-foreground"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession('')}
            title="プロンプトなしでセッションとターミナルだけを起動します"
          >
            空で起動
          </Button>

          <Button
            size="sm"
            class="gap-1.5 h-8 text-xs font-medium"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession()}
          >
            {#if busy}
              <span>起動中…</span>
            {:else}
              <PlayIcon class="size-3.5 fill-current" />
              <span>セッション開始</span>
              <kbd class="bg-primary-foreground/20 text-[10px] px-1 py-0.5 rounded font-mono">↵</kbd>
            {/if}
          </Button>
        </div>
      </div>
    </div>

    <div class="flex flex-col gap-2">
      <span class="text-muted-foreground text-xs">おすすめのプロンプト:</span>
      <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
        {#each suggestionPrompts as suggestion}
          <button
            type="button"
            class="hover:bg-accent/60 bg-muted/30 text-muted-foreground hover:text-foreground flex items-center justify-between rounded-lg border p-2.5 text-left text-xs transition-colors"
            onclick={() => {
              promptText = suggestion;
            }}
          >
            <span class="line-clamp-1">{suggestion}</span>
            <CornerDownLeftIcon class="size-3 shrink-0 opacity-60" />
          </button>
        {/each}
      </div>
    </div>
  </div>
</div>
