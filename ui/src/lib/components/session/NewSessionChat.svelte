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
  import { formatRelativeTime } from '$lib/format';
  import {
    buildPathCandidates,
    pickDefaultNode,
    type PathCandidate
  } from '$lib/new-session';
  import { getNodeAvailability } from '$lib/node-status';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import { onMount, untrack } from 'svelte';
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

  // 既定解決の制御: URL パラメータ・ノード選択の初回適用を追跡する
  let nodeTouched = $state(false);
  let urlProjectApplied = $state(false);
  let urlNodeApplied = $state(false);
  let urlAgentApplied = $state(false);
  let lastAutoPathKey = '';

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

  // URL の `project` は初回のみ適用する (以降の手動選択を上書きしない)
  $effect(() => {
    if (urlProjectApplied || projects.length === 0) return;
    const urlProject = page.url.searchParams.get('project');
    selectedProjectId =
      urlProject && projects.some((p) => p.project_id === urlProject)
        ? urlProject
        : selectedProjectId || projects[0].project_id;
    urlProjectApplied = true;
  });

  // プロジェクトの実行履歴があるオンラインノードを既定にする
  // (ユーザーがノードを選んだ後は追従しない)
  $effect(() => {
    if (nodeTouched || nodes.length === 0) return;
    const defaultNode = untrack(() => pickDefaultNode(nodes, selectedProject, sync.sessions));
    if (defaultNode !== '' && defaultNode !== nodeId) nodeId = defaultNode;
  });

  // URL の `node` / `path` (ディープリンク) は初回のみ適用する
  $effect(() => {
    if (urlNodeApplied || nodes.length === 0) return;
    const urlNode = page.url.searchParams.get('node');
    if (urlNode && nodes.some((n) => n.node_id === urlNode)) {
      nodeId = urlNode;
      nodeTouched = true;
    }
    const urlPath = page.url.searchParams.get('path');
    if (urlPath) {
      localPath = urlPath;
      useCustomPath = true;
    }
    urlNodeApplied = true;
  });

  // 実行ディレクトリ候補 (紐付け + 直近セッション) を直近使用順に解決する
  const pathCandidates = $derived.by(() =>
    buildPathCandidates(selectedProject, nodeId, sync.sessions)
  );
  const selectedCandidate = $derived(
    pathCandidates.find((candidate) => candidate.path === localPath) ?? null
  );
  // このノードに候補が無い場合の切り替え候補 (登録がある別ノード)
  const otherNodesWithCandidates = $derived(
    nodes.filter(
      (node) =>
        node.node_id !== nodeId &&
        buildPathCandidates(selectedProject, node.node_id, sync.sessions).length > 0
    )
  );

  // 候補の既定選択: プロジェクト × ノードが変わったとき、または現在値が候補から
  // 消えたときのみ更新する (セッション更新で候補順が変わっても選択は維持する)
  $effect(() => {
    if (isProvisionerMode || useCustomPath) return;
    const key = `${selectedProject?.project_id ?? ''}|${nodeId}`;
    const containsCurrent = pathCandidates.some((candidate) => candidate.path === localPath);
    if (
      key === lastAutoPathKey &&
      (containsCurrent || (localPath === '' && pathCandidates.length === 0))
    ) {
      return;
    }
    lastAutoPathKey = key;
    if (!containsCurrent) localPath = pathCandidates[0]?.path ?? '';
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

  // 未指定の理由をユーザーに明示する (無言の disabled を避ける)
  const pathMissing = $derived(
    !isProvisionerMode &&
      (pathMode === 'existing' ? localPath.trim() === '' : branch.trim() === '')
  );
  const pathRequiredHint = $derived(
    pathMode === 'existing' ? '実行ディレクトリを選択してください' : '新ブランチ名を入力してください'
  );

  // URL の `fork_session` / `agent` は初回のみ適用する
  $effect(() => {
    if (urlAgentApplied || nodes.length === 0) return;
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
    urlAgentApplied = true;
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

  /** プロジェクト変更時: 既定ノード・既定パスを再解決する。 */
  function onProjectChange() {
    nodeTouched = false;
    useCustomPath = false;
  }

  /** ノード変更時: パスはノードごとに異なるため既定パスを再解決する。 */
  function onNodeChange() {
    nodeTouched = true;
    useCustomPath = false;
  }

  /** 実行ディレクトリ候補の表示ラベル。 */
  function candidateLabel(candidate: PathCandidate): string {
    const parts: string[] = [];
    if (candidate.isWorktree) parts.push('[Worktree]');
    if (candidate.gitBranch) parts.push(`[${candidate.gitBranch}]`);
    parts.push(candidate.path);
    const suffix = candidate.source === 'session' ? ' (前回のセッション)' : '';
    return `${parts.join(' ')} · ${formatRelativeTime(candidate.usedAt)}${suffix}`;
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
              onchange={onProjectChange}
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
              onchange={onNodeChange}
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

      {#if !isProvisionerMode && projects.length > 0}
        <div class="flex flex-col gap-1.5 border-t pt-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <Label class="text-muted-foreground flex items-center gap-1.5 text-xs">
              <FolderGit2Icon class="size-3.5" />
              実行ディレクトリ
              <span class="text-destructive">*</span>
            </Label>
            <div class="flex items-center gap-1">
              <button
                type="button"
                class="rounded-full border px-2.5 py-0.5 text-xs transition-colors {pathMode === 'existing'
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'text-muted-foreground hover:bg-muted'}"
                onclick={() => (pathMode = 'existing')}
              >
                既存
              </button>
              <button
                type="button"
                class="rounded-full border px-2.5 py-0.5 text-xs transition-colors {pathMode === 'new_worktree'
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'text-muted-foreground hover:bg-muted'}"
                onclick={() => (pathMode = 'new_worktree')}
              >
                新規 Worktree
              </button>
            </div>
          </div>

          {#if pathMode === 'existing'}
            <div class="flex items-center gap-1.5">
              {#if pathCandidates.length > 0 && !useCustomPath}
                <select
                  bind:value={localPath}
                  aria-invalid={localPath.trim() === ''}
                  class="border-input bg-background focus-visible:ring-ring/40 h-8 min-w-0 flex-1 rounded-md border px-2 text-xs outline-none focus-visible:ring-1 {localPath.trim() ===
                  ''
                    ? 'border-destructive'
                    : ''}"
                >
                  {#each pathCandidates as candidate (candidate.path)}
                    <option value={candidate.path}>{candidateLabel(candidate)}</option>
                  {/each}
                </select>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                  onclick={() => {
                    useCustomPath = true;
                    browserOpen = true;
                  }}
                  title="フォルダブラウザで別のディレクトリを選択"
                >
                  <FolderOpenIcon class="size-3.5" />
                  <span>参照…</span>
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  class="text-muted-foreground hover:text-foreground h-8 shrink-0 px-2 text-xs"
                  onclick={() => (useCustomPath = true)}
                  title="パスを直接入力する"
                >
                  直接入力
                </Button>
              {:else}
                <Input
                  bind:value={localPath}
                  aria-invalid={localPath.trim() === ''}
                  placeholder="絶対パスを入力 (例: /home/user/project or C:\repo)"
                  class="h-8 min-w-0 flex-1 font-mono text-xs"
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                  disabled={!nodeId}
                  onclick={() => (browserOpen = true)}
                  title="フォルダブラウザで選択"
                >
                  <FolderOpenIcon class="size-3.5" />
                  <span>参照…</span>
                </Button>
                {#if pathCandidates.length > 0}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    class="text-muted-foreground hover:text-foreground h-8 shrink-0 px-2 text-xs"
                    onclick={() => {
                      useCustomPath = false;
                      if (pathCandidates[0]) localPath = pathCandidates[0].path;
                    }}
                    title="登録済みパスの一覧に戻る"
                  >
                    一覧に戻る
                  </Button>
                {/if}
              {/if}
            </div>

            {#if selectedCandidate}
              <p class="text-muted-foreground text-[11px]">
                {selectedCandidate.source === 'binding' ? '登録済み' : '前回のセッションで使用'} · 最終使用
                {formatRelativeTime(selectedCandidate.usedAt)}
                {#if selectedCandidate.isWorktree}
                  · Worktree
                {/if}
                {#if selectedCandidate.gitBranch}
                  · {selectedCandidate.gitBranch}
                {/if}
              </p>
            {:else if localPath.trim() !== ''}
              <p class="text-muted-foreground text-[11px]">
                未登録のパスです (セッション開始時に自動登録されます)
              </p>
            {/if}

            {#if pathCandidates.length === 0 && !(useCustomPath && localPath.trim() !== '')}
              <div
                class="flex flex-col gap-1.5 rounded-md border border-amber-500/40 bg-amber-500/10 p-2.5 text-[11px]"
              >
                <span>
                  このノードでは {selectedProject?.name ?? 'このプロジェクト'} の実行ディレクトリが未登録です。フォルダを指定するか、プロジェクト設定でスキャンしてください。
                </span>
                <div class="flex flex-wrap items-center gap-1.5">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    class="h-7 px-2 text-[11px]"
                    onclick={() => {
                      useCustomPath = true;
                      browserOpen = true;
                    }}
                  >
                    フォルダを選択
                  </Button>
                  <a
                    href="/projects"
                    class="text-muted-foreground hover:text-foreground underline underline-offset-2"
                  >
                    プロジェクト設定へ
                  </a>
                  {#each otherNodesWithCandidates as otherNode (otherNode.node_id)}
                    <button
                      type="button"
                      class="text-muted-foreground hover:text-foreground underline underline-offset-2"
                      onclick={() => {
                        nodeId = otherNode.node_id;
                        nodeTouched = true;
                      }}
                    >
                      {otherNode.name} に登録あり
                    </button>
                  {/each}
                </div>
              </div>
            {/if}
          {:else}
            <Input
              bind:value={branch}
              aria-invalid={branch.trim() === ''}
              placeholder="feat/new-task"
              class="h-8 text-xs"
            />
            <p class="text-muted-foreground text-[11px]">
              新しい独立ブランチを作成して Worktree で実行します (起点ブランチ・配置先は詳細設定)
            </p>
          {/if}
        </div>
      {/if}

      <div>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
          onclick={() => (showAdvanced = !showAdvanced)}
        >
          <Settings2Icon class="size-3" />
          <span>{showAdvanced ? '詳細設定を閉じる' : '詳細設定'}</span>
        </button>

        {#if showAdvanced}
          <div class="border-t pt-3 mt-2 flex flex-col gap-3">
            {#if isProvisionerMode}
              <div class="flex flex-col gap-1">
                <Label class="text-xs">クローンするブランチ (任意)</Label>
                <Input bind:value={branch} placeholder="main (省略時はリモートの既定ブランチ)" class="h-8 text-xs" />
              </div>
            {:else if pathMode === 'new_worktree'}
              <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <div class="flex flex-col gap-1">
                  <Label class="text-xs">起点ブランチ (任意)</Label>
                  <Input bind:value={baseBranch} placeholder="main" class="h-8 text-xs" />
                </div>
                <div class="flex flex-col gap-1">
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

            <div
              class="{isProvisionerMode || pathMode === 'new_worktree'
                ? 'border-t pt-2.5 '
                : ''}grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs"
            >
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
          {#if pathMissing}
            <span class="text-destructive text-xs">{pathRequiredHint}</span>
          {/if}
          <Button
            variant="ghost"
            size="sm"
            class="text-xs h-8 text-muted-foreground hover:text-foreground"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession('')}
            title={pathMissing ? pathRequiredHint : 'プロンプトなしでセッションとターミナルだけを起動します'}
          >
            空で起動
          </Button>

          <Button
            size="sm"
            class="gap-1.5 h-8 text-xs font-medium"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession()}
            title={pathMissing ? pathRequiredHint : 'セッションを開始します'}
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
