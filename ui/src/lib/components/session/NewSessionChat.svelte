<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import FolderBrowserDialog from '$lib/components/fs/FolderBrowserDialog.svelte';
  import type { ProvisionerSummary } from '$lib/generated/ProvisionerSummary';
  import type { ProvisionerTestResponse } from '$lib/generated/ProvisionerTestResponse';
  import { getNodeAvailability } from '$lib/node-status';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import { onMount } from 'svelte';
  import BotIcon from '@lucide/svelte/icons/bot';
  import BoxIcon from '@lucide/svelte/icons/box';
  import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
  import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';
  import FolderGit2Icon from '@lucide/svelte/icons/folder-git-2';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import GitForkIcon from '@lucide/svelte/icons/git-fork';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import PlayIcon from '@lucide/svelte/icons/play';
  import ServerIcon from '@lucide/svelte/icons/server';
  import Settings2Icon from '@lucide/svelte/icons/settings-2';
  import SparklesIcon from '@lucide/svelte/icons/sparkles';
  import XCircleIcon from '@lucide/svelte/icons/x-circle';
  import XIcon from '@lucide/svelte/icons/x';

  type PathMode = 'existing' | 'new_worktree';
  type RunTarget = 'node' | 'provisioner';

  let selectedProjectId = $state('');
  let nodeId = $state('');
  let runTarget = $state<RunTarget>('node');
  let provisionerName = $state('');
  const provisioners = $state<ProvisionerSummary[]>([]);
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
  let browserOpen = $state(false);
  let newWorktreeBrowserOpen = $state(false);
  let useCustomPath = $state(false);

  // Fork 引き継ぎ
  let forkSessionId = $state('');
  let forkNodeSeq = $state<number | null>(null);

  // 起動オプション
  let sessionMode = $state<'default' | 'code' | 'plan'>('default');
  let opencodeMode = $state<'default' | 'bridge' | 'acp'>('default');
  let extraArgsText = $state('');

  // プロビジョナー接続テスト
  let testBusy = $state(false);
  let testResult = $state<ProvisionerTestResponse | null>(null);
  let testDialogOpen = $state(false);

  const projects = $derived(sync.projects);
  const selectedProject = $derived(
    projects.find((p) => p.project_id === selectedProjectId) ?? projects[0] ?? null
  );

  const nodes = $derived(sync.nodes);
  const selectedNode = $derived(nodes.find((n) => n.node_id === nodeId) ?? null);
  const installedAgents = $derived(selectedNode?.installed_agents ?? []);

  // 一時VMプロビジョナー定義 (中央サーバー接続時のみ取得できる)
  onMount(async () => {
    try {
      const list = await sync.connection.client.provisioners();
      provisioners.splice(0, provisioners.length, ...list);
      if (list.length > 0 && provisionerName === '') provisionerName = list[0].name;
    } catch {
      // ローカルノード接続時はプロビジョナーが存在しないため無視する
    }
  });

  const isProvisionerMode = $derived(runTarget === 'provisioner');
  const selectedProvisioner = $derived(
    provisioners.find((p) => p.name === provisionerName) ?? provisioners[0] ?? null
  );

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
    if (!useCustomPath && nodePaths.length > 0 && !nodePaths.some((opt) => opt.path === localPath)) {
      localPath = nodePaths[0].path;
    }
  });

  const effectiveAgent = $derived(
    isProvisionerMode
      ? customAgent.trim() || 'opencode2'
      : installedAgents.length > 0
        ? agentId || installedAgents[0]
        : customAgent.trim()
  );

  const canSubmit = $derived(
    !busy &&
      selectedProject !== null &&
      effectiveAgent !== '' &&
      (isProvisionerMode
        ? selectedProvisioner !== null
        : nodeId !== '' && (pathMode === 'existing' ? localPath !== '' : branch.trim() !== ''))
  );

  $effect(() => {
    const forkSession = page.url.searchParams.get('fork_session');
    if (forkSession) {
      forkSessionId = forkSession;
      const forkSeqStr = page.url.searchParams.get('fork_seq');
      forkNodeSeq = forkSeqStr ? Number.parseInt(forkSeqStr, 10) : null;
    }
    const agentParam = page.url.searchParams.get('agent');
    if (agentParam) {
      if (installedAgents.includes(agentParam)) {
        agentId = agentParam;
      } else {
        customAgent = agentParam;
      }
    }
  });

  async function runProvisionerTest(name: string) {
    if (testBusy || !name) return;
    testBusy = true;
    testResult = null;
    testDialogOpen = true;
    try {
      const result = await sync.connection.client.testProvisioner(name);
      testResult = result;
      if (result.ok) {
        toast.success(`プロビジョナー '${name}' の疎通テストに成功しました`);
      } else {
        toast.error(`プロビジョナー '${name}' の疎通テストに失敗しました: ${result.error ?? '不明なエラー'}`);
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      testResult = {
        name,
        ok: false,
        node_id: null,
        log_lines: [],
        error: error instanceof Error ? error.message : String(error)
      };
    } finally {
      testBusy = false;
    }
  }

  async function handleStartSession(initialPrompt?: string) {
    if (!canSubmit || !selectedProject) return;
    busy = true;
    const promptToSend = (initialPrompt ?? promptText).trim();
    const useProvisioner = isProvisionerMode && selectedProvisioner !== null;
    const extraArgs = extraArgsText.trim() ? extraArgsText.trim().split(/\s+/) : null;

    try {
      const response = await sync.connection.client.createSession({
        command_id: crypto.randomUUID(),
        project_id: selectedProject.project_id,
        node_id: useProvisioner ? null : nodeId,
        provisioner: useProvisioner ? selectedProvisioner.name : null,
        local_path: useProvisioner || pathMode === 'new_worktree' ? null : localPath,
        worktree: useProvisioner
          ? branch.trim() === ''
            ? null
            : { branch: branch.trim(), base_branch: null, new_path: null }
          : pathMode === 'new_worktree'
            ? {
                branch: branch.trim(),
                base_branch: baseBranch.trim() === '' ? null : baseBranch.trim(),
                new_path: newPath.trim() === '' ? null : newPath.trim()
              }
            : null,
        agent_id: effectiveAgent,
        initial_prompt: promptToSend === '' ? null : promptToSend,
        mode: sessionMode === 'default' ? null : sessionMode,
        opencode_mode: opencodeMode === 'default' ? null : opencodeMode,
        extra_args: extraArgs,
        fork: forkSessionId
          ? {
              from_session_id: forkSessionId,
              from_node_seq: forkNodeSeq,
              restore_git_bundle_b64: null
            }
          : null
      });

      toast.success(
        useProvisioner
          ? '一時VMの起動を開始しました (ブートストラップの進捗はセッション画面に表示されます)'
          : forkSessionId
            ? 'フォークセッションを開始しました'
            : 'セッションを開始しました'
      );
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

    {#if forkSessionId}
      <div class="bg-primary/10 border-primary/20 flex items-center justify-between rounded-xl border p-3 text-xs">
        <div class="flex items-center gap-2">
          <GitForkIcon class="text-primary size-4 shrink-0" />
          <span>
            セッション <code class="font-mono bg-background/50 px-1 py-0.5 rounded">{forkSessionId.slice(0, 8)}</code>
            {#if forkNodeSeq !== null}
              (シーケンス #{forkNodeSeq})
            {/if}
            から分岐して新しいセッションを開始します。
          </span>
        </div>
        <Button
          variant="ghost"
          size="sm"
          class="h-6 px-2 text-xs"
          onclick={() => {
            forkSessionId = '';
            forkNodeSeq = null;
          }}
        >
          <XIcon class="size-3.5 mr-1" />
          フォーク解除
        </Button>
      </div>
    {/if}

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
            {#if isProvisionerMode}
              <BoxIcon class="size-3.5" />
              一時VMプロビジョナー
            {:else}
              <ServerIcon class="size-3.5" />
              実行ノード
            {/if}
          </Label>
          {#if isProvisionerMode}
            <div class="flex items-center gap-1.5">
              {#if provisioners.length > 0}
                <select
                  bind:value={provisionerName}
                  class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1 flex-1 min-w-0"
                >
                  {#each provisioners as provisioner (provisioner.name)}
                    <option value={provisioner.name}>
                      {provisioner.description ?? provisioner.name}
                    </option>
                  {/each}
                </select>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 px-2 text-xs shrink-0"
                  disabled={!provisionerName || testBusy}
                  onclick={() => runProvisionerTest(provisionerName)}
                  title="プロビジョナーの起動・接続テストを実行"
                >
                  {#if testBusy}
                    <LoaderCircleIcon class="size-3.5 animate-spin" />
                  {:else}
                    テスト
                  {/if}
                </Button>
              {:else}
                <span class="text-muted-foreground text-xs leading-8">
                  プロビジョナー未定義 (中央サーバーの config.toml で定義してください)
                </span>
              {/if}
            </div>
          {:else}
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
          {/if}
        </div>

        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            <BotIcon class="size-3.5" />
            エージェント
          </Label>
          {#if !isProvisionerMode && installedAgents.length > 0}
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
              placeholder={isProvisionerMode ? 'opencode2 (一時VMに自動導入)' : 'opencode2 / acp'}
              class="h-8 text-xs"
            />
          {/if}
        </div>
      </div>

      <div class="flex items-center gap-1.5 text-xs">
        <span class="text-muted-foreground">実行環境:</span>
        <button
          type="button"
          class="rounded-full border px-2.5 py-0.5 transition-colors {runTarget === 'node'
            ? 'bg-primary text-primary-foreground border-primary'
            : 'text-muted-foreground hover:bg-muted'}"
          onclick={() => (runTarget = 'node')}
        >
          常駐ノード
        </button>
        <button
          type="button"
          class="rounded-full border px-2.5 py-0.5 transition-colors {isProvisionerMode
            ? 'bg-primary text-primary-foreground border-primary'
            : 'text-muted-foreground hover:bg-muted'}"
          onclick={() => (runTarget = 'provisioner')}
          disabled={provisioners.length === 0}
          title={provisioners.length === 0
            ? 'プロビジョナーは中央サーバーの config.toml で定義します'
            : '使い捨ての隔離環境 (Docker / Incus / Colab 等) を起動します'}
        >
          一時VM (隔離)
        </button>
        {#if isProvisionerMode}
          <span class="text-muted-foreground">
            · 使い捨て環境で実行し、終了時に git bundle を退避します
          </span>
        {/if}
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
            {#if isProvisionerMode}
              <div class="flex flex-col gap-1">
                <Label class="text-xs">クローンするブランチ (任意)</Label>
                <Input bind:value={branch} placeholder="main (省略時はリモートの既定ブランチ)" class="h-8 text-xs" />
              </div>
            {:else}
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
                <div class="flex flex-col gap-1.5">
                  <div class="flex items-center gap-1.5">
                    {#if nodePaths.length > 0 && !useCustomPath}
                      <select
                        bind:value={localPath}
                        class="border-input bg-background h-8 rounded-md border px-2 text-xs flex-1 min-w-0"
                      >
                        {#each nodePaths as opt (opt.path)}
                          <option value={opt.path}>{opt.label}</option>
                        {/each}
                      </select>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-8 px-2.5 text-xs shrink-0 gap-1.5"
                        disabled={!nodeId}
                        onclick={() => {
                          useCustomPath = true;
                          browserOpen = true;
                        }}
                        title="フォルダブラウザで別のディレクトリを選択"
                      >
                        <FolderOpenIcon class="size-3.5" />
                        <span>フォルダ参照…</span>
                      </Button>
                    {:else}
                      <Input
                        bind:value={localPath}
                        placeholder="絶対パスを入力 (例: /home/user/project or C:\repo)"
                        class="h-8 text-xs font-mono flex-1 min-w-0"
                      />
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-8 px-2.5 text-xs shrink-0 gap-1.5"
                        disabled={!nodeId}
                        onclick={() => (browserOpen = true)}
                        title="フォルダブラウザで選択"
                      >
                        <FolderOpenIcon class="size-3.5" />
                        <span>参照…</span>
                      </Button>
                      {#if nodePaths.length > 0}
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          class="h-8 px-2 text-xs shrink-0 text-muted-foreground hover:text-foreground"
                          onclick={() => {
                            useCustomPath = false;
                            if (nodePaths[0]) localPath = nodePaths[0].path;
                          }}
                          title="登録済みパスの一覧に戻る"
                        >
                          一覧に戻る
                        </Button>
                      {/if}
                    {/if}
                  </div>
                </div>
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
                  <div class="flex flex-col gap-1 sm:col-span-2">
                    <Label class="text-xs">配置先パス (任意・省略時は既定テンプレート)</Label>
                    <div class="flex items-center gap-1.5">
                      <Input
                        bind:value={newPath}
                        placeholder="配置先の絶対パス (例: /home/user/wt or C:\repo\.fxg\wt)"
                        class="h-8 text-xs font-mono flex-1 min-w-0"
                      />
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        class="h-8 px-2.5 text-xs shrink-0 gap-1.5"
                        disabled={!nodeId}
                        onclick={() => (newWorktreeBrowserOpen = true)}
                        title="フォルダブラウザで配置先ディレクトリを選択"
                      >
                        <FolderOpenIcon class="size-3.5" />
                        <span>参照…</span>
                      </Button>
                    </div>
                  </div>
                </div>
              {/if}
            {/if}

            <div class="border-t pt-2.5 grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs">
              <div class="flex flex-col gap-1">
                <Label class="text-xs">エージェント初期モード</Label>
                <select
                  bind:value={sessionMode}
                  class="border-input bg-background h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
                >
                  <option value="default">既定 (未指定)</option>
                  <option value="code">code (コード実装・編集)</option>
                  <option value="plan">plan (設計・プランニング)</option>
                </select>
              </div>

              <div class="flex flex-col gap-1">
                <Label class="text-xs">OpenCode2 実行モード</Label>
                <select
                  bind:value={opencodeMode}
                  class="border-input bg-background h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
                >
                  <option value="default">既定 (TUI + Server ブリッジ)</option>
                  <option value="bridge">bridge (サーバーブリッジのみ)</option>
                  <option value="acp">acp (ACP Driver 経由)</option>
                </select>
              </div>

              <div class="flex flex-col gap-1 sm:col-span-2">
                <Label class="text-xs">追加 CLI 引数 (スペース区切り)</Label>
                <Input
                  bind:value={extraArgsText}
                  placeholder="例: --model claude-3-7-sonnet --verbose"
                  class="h-8 text-xs font-mono"
                />
              </div>
            </div>
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
          {#if isProvisionerMode}
            {#if selectedProvisioner}
              <span class="text-muted-foreground text-xs">
                {selectedProvisioner.name}
                {#if selectedProvisioner.idle_timeout_secs}
                  · idle {Math.round(selectedProvisioner.idle_timeout_secs / 60)}分で自動破棄
                {/if}
              </span>
            {/if}
          {:else if selectedNode}
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

<FolderBrowserDialog
  bind:open={browserOpen}
  {nodeId}
  initialPath={localPath}
  onSelect={(path) => {
    localPath = path;
    useCustomPath = true;
  }}
/>

<FolderBrowserDialog
  bind:open={newWorktreeBrowserOpen}
  {nodeId}
  initialPath={newPath || localPath}
  title="Worktree 配置先フォルダを選択"
  onSelect={(path) => {
    newPath = path;
  }}
/>

<Dialog.Root bind:open={testDialogOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>プロビジョナー疎通テスト: {provisionerName}</Dialog.Title>
      <Dialog.Description>
        一時VMを実際に起動し、ノード接続と終了処理が正常に行えるか検証します。
      </Dialog.Description>
    </Dialog.Header>
    <div class="py-2 text-xs flex flex-col gap-3">
      {#if testBusy}
        <div class="flex flex-col items-center justify-center gap-2 py-8 text-muted-foreground">
          <LoaderCircleIcon class="size-6 animate-spin text-primary" />
          <span>プロビジョナーを起動しています… (数十秒かかる場合があります)</span>
        </div>
      {:else if testResult}
        <div class="flex items-center gap-2 font-medium">
          {#if testResult.ok}
            <CheckCircle2Icon class="size-5 text-green-500" />
            <span class="text-green-600 dark:text-green-400">疎通テスト成功</span>
            {#if testResult.node_id}
              <span class="text-muted-foreground font-mono text-[11px]">({testResult.node_id})</span>
            {/if}
          {:else}
            <XCircleIcon class="size-5 text-destructive" />
            <span class="text-destructive">疎通テスト失敗</span>
          {/if}
        </div>

        {#if testResult.error}
          <div class="bg-destructive/10 text-destructive border-destructive/20 rounded-md border p-2.5 font-mono text-[11px] whitespace-pre-wrap">
            {testResult.error}
          </div>
        {/if}

        {#if testResult.log_lines && testResult.log_lines.length > 0}
          <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-[11px]">実行ログ:</span>
            <div class="bg-muted max-h-48 overflow-y-auto rounded-md p-2 font-mono text-[11px] leading-tight text-foreground whitespace-pre-wrap">
              {testResult.log_lines.join('\n')}
            </div>
          </div>
        {/if}
      {/if}
    </div>
    <Dialog.Footer>
      <Button variant="outline" size="sm" onclick={() => (testDialogOpen = false)}>
        閉じる
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
