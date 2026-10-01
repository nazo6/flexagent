<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import BootstrapLogCard from '$lib/components/BootstrapLogCard.svelte';
  import ChatTimeline from '$lib/components/chat/ChatTimeline.svelte';
  import Composer from '$lib/components/chat/Composer.svelte';
  import DiffPane from '$lib/components/diff/DiffPane.svelte';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import SessionStatusBadge from '$lib/components/SessionStatusBadge.svelte';
  import TerminalView from '$lib/components/terminal/TerminalView.svelte';
  import { Button } from '$lib/components/ui/button';
  import { formatRelativeTime } from '$lib/format';
  import { shortId } from '$lib/session-status';
  import { sync } from '$lib/stores/app.svelte';
  import {
    bootstrapLogLines,
    buildTimelineItems,
    capabilitiesFromEvents,
    latestStatusFromEvents
  } from '$lib/sync/reducer';
  import { cn } from '$lib/utils';
  import { toast } from 'svelte-sonner';
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import FileCodeIcon from '@lucide/svelte/icons/file-code';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import GitForkIcon from '@lucide/svelte/icons/git-fork';
  import MessageSquareIcon from '@lucide/svelte/icons/message-square';
  import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SkullIcon from '@lucide/svelte/icons/skull';
  import SquareTerminalIcon from '@lucide/svelte/icons/square-terminal';
  import Undo2Icon from '@lucide/svelte/icons/undo-2';

  type ViewMode = 'chat' | 'diff' | 'terminal';

  const sessionId = $derived(page.params.id ?? '');
  const session = $derived(sync.sessions.find((entry) => entry.session_id === sessionId) ?? null);
  const sessionNode = $derived(
    session ? (sync.nodes.find((n) => n.node_id === session.node_id) ?? null) : null
  );

  const timeline = $derived(sync.timelineFor(sessionId));
  const currentStatus = $derived(
    latestStatusFromEvents(timeline.events)?.status ?? session?.status ?? null
  );
  const items = $derived(
    buildTimelineItems({
      events: timeline.events,
      messageDeltas: timeline.messageDeltas,
      thoughtDeltas: timeline.thoughtDeltas,
      toolProgress: timeline.toolProgress,
      terminalDeltas: timeline.terminalDeltas,
      pendingPrompts: sync.pendingPrompts
    })
  );
  const capabilities = $derived(capabilitiesFromEvents(timeline.events));
  // 永続イベント (ターン完了後に同期された BootstrapLog) と、
  // サーバーが一時VMから配信するエフェメラルログを統合する
  const bootstrapLines = $derived.by(() => {
    const persisted = bootstrapLogLines(timeline.events);
    const live = sync.bootstrapLinesFor(sessionId);
    return live.length === 0 ? persisted : [...new Set([...persisted, ...live])];
  });

  let chatContainer = $state<HTMLDivElement | null>(null);
  let loadedFor = $state<string | null>(null);
  let showScrollBottom = $state(false);

  // 表示モード (モバイル用の切り替えタブ、およびデスクトップ用の右ペイン選択)
  let activeTab = $state<ViewMode>('chat');
  let sidePanelMode = $state<'diff' | 'terminal' | null>(null);

  // セッションを WS の優先配信対象にし、履歴未取得なら REST でフォールバック読込
  $effect(() => {
    const id = sessionId;
    if (!id) return;
    sync.setActiveSession(id);
    if (loadedFor !== id) {
      loadedFor = id;
      const target = sync.timelineFor(id);
      if (target.events.length === 0) {
        sync.loadSessionEvents(id).catch(() => {
          /* WS のリプレイが到着すれば表示できるため無視する */
        });
      }
    }
    return () => sync.setActiveSession(null);
  });

  // 新規アイテム追加時に末尾へスクロール (ユーザーが過去を閲覧中でなければ)
  $effect(() => {
    const currentItems = items;
    if (chatContainer !== null && currentItems.length > 0 && !showScrollBottom) {
      chatContainer.scrollTop = chatContainer.scrollHeight;
    }
  });

  function handleScroll() {
    if (!chatContainer) return;
    const distanceToBottom =
      chatContainer.scrollHeight - chatContainer.scrollTop - chatContainer.clientHeight;
    showScrollBottom = distanceToBottom > 150;
  }

  let killDialogOpen = $state(false);
  let killing = $state(false);

  // Resume (停止済みセッションの再開) の実行状態
  let resuming = $state(false);

  /** 一時VMの先行作成中セッションを除き、Resume を表示する。
   * 強制終了時は DB の status が idle / running のまま残るため、
   * 一見実行中でも Resume を出し、実行中だった場合はノード側で拒否させる。 */
  const canResume = $derived(
    session !== null && session.status !== 'provisioning' && session.status !== 'bootstrapping'
  );

  // Revert (Shadow Git Tree 巻き戻し) 確認ダイアログの状態
  let revertSeq = $state<number | null>(null);
  let reverting = $state(false);

  function scrollToBottom() {
    if (!chatContainer) return;
    chatContainer.scrollTo({ top: chatContainer.scrollHeight, behavior: 'smooth' });
    showScrollBottom = false;
  }

  function toggleSidePanel(mode: 'diff' | 'terminal') {
    if (sidePanelMode === mode) {
      sidePanelMode = null;
    } else {
      sidePanelMode = mode;
    }
  }

  function handleForkSession() {
    if (!sessionId) return;
    const params = new URLSearchParams();
    params.set('fork_session', sessionId);
    if (session?.project_id) params.set('project', session.project_id);
    if (session?.agent_id) params.set('agent', session.agent_id);
    if (session?.last_node_seq) params.set('fork_seq', String(session.last_node_seq));
    void goto(`/?${params.toString()}`);
  }

  /** 停止済みセッションを再開する (`POST /sessions/:id/resume`)。 */
  async function handleResumeSession() {
    if (!sessionId || resuming) return;
    resuming = true;
    try {
      const result = await sync.resumeSession(sessionId);
      if (result.context_restored) {
        toast.success('セッションを再開しました (エージェントのコンテキストを復元)');
      } else {
        toast.success('セッションを再開しました (履歴を引き継いで継続)');
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      resuming = false;
    }
  }

  async function handleKillSession() {
    if (!sessionId || killing) return;
    killing = true;
    try {
      const result = await sync.controlSession(sessionId, { action: 'kill' });
      if (result.success) {
        toast.success('セッションの子プロセスツリーを強制終了しました');
        killDialogOpen = false;
        await sync.refreshSessions();
      } else {
        toast.error(result.error ?? result.code ?? '強制終了に失敗しました');
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      killing = false;
    }
  }

  /** タイムラインの「この時点へ巻き戻す」から確認ダイアログを開く。 */
  function requestRevert(seq: number) {
    revertSeq = seq;
  }

  /** 指定ターン時点へワークスペースを巻き戻す (`POST /sessions/:id/revert`)。 */
  async function handleRevertSession() {
    if (!sessionId || revertSeq === null || reverting) return;
    reverting = true;
    try {
      const result = await sync.revertSession(sessionId, revertSeq);
      toast.success(
        `node_seq=${result.target_node_seq} 時点へ巻き戻しました (復元 ${result.restored_files} 件 / 削除 ${result.removed_files} 件)`
      );
      revertSeq = null;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      reverting = false;
    }
  }
</script>

<svelte:head>
  <title>{session?.title ?? 'セッション'} - FlexAgent</title>
</svelte:head>

<div class="flex h-full w-full flex-col overflow-hidden bg-background">
  <!-- セッションヘッダー -->
  <header class="bg-card/80 border-b px-3 py-2 shrink-0 backdrop-blur">
    <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5">
      <!-- 左側: 戻るリンク + タイトル + ノード状態 -->
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <a
          href="/"
          class="hover:bg-accent text-muted-foreground hover:text-foreground -ml-1 flex size-7 items-center justify-center rounded-md border md:hidden"
          title="セッション一覧に戻る"
        >
          <ChevronLeftIcon class="size-4" />
        </a>

        <div class="flex min-w-0 flex-col gap-0.5">
          <div class="flex items-center gap-2">
            <h1 class="truncate text-sm font-semibold tracking-tight">
              {session?.title ?? 'セッション'}
            </h1>
            {#if currentStatus}
              <SessionStatusBadge status={currentStatus} />
            {/if}
          </div>

          <div class="text-muted-foreground flex flex-wrap items-center gap-x-2 text-[11px]">
            <!-- ノード利用可能ステータス -->
            <NodeStatusBadge node={sessionNode} showNodeName={true} />
            <span class="opacity-40">/</span>
            <span class="truncate">{session?.project_id}</span>
            {#if session?.git_branch}
              <span class="opacity-40">/</span>
              <span class="font-mono">{session.git_branch}</span>
            {/if}
            {#if session?.is_worktree}
              <span class="rounded bg-muted px-1 text-[10px]">wt</span>
            {/if}
            {#if session?.local_path}
              <span class="opacity-40">/</span>
              <button
                type="button"
                class="hover:text-foreground inline-flex items-center gap-1 font-mono text-[11px] truncate max-w-44 sm:max-w-xs md:max-w-md transition-colors cursor-pointer"
                title={`ワーキングディレクトリ: ${session.local_path} (クリックでコピー)`}
                onclick={() => {
                  navigator.clipboard.writeText(session.local_path);
                  toast.success('ワーキングディレクトリをコピーしました');
                }}
              >
                <FolderIcon class="size-3 shrink-0 text-amber-500/80" />
                <span class="truncate">{session.local_path}</span>
              </button>
            {/if}
            <span class="opacity-40">/</span>
            <span>{session?.agent_id}</span>
            {#if session?.updated_at}
              <span class="opacity-40">/</span>
              <span>{formatRelativeTime(session.updated_at)}</span>
            {/if}
          </div>
        </div>
      </div>

      <!-- 右側: モード切替 / ペイン開閉ボタン -->
      <div class="flex items-center gap-1.5">
        <!-- モバイル向け: タブ切り替えボタン -->
        <div class="flex rounded-md border bg-muted/30 p-0.5 md:hidden">
          <button
            type="button"
            class={cn(
              'px-2.5 py-1 text-xs font-medium rounded',
              activeTab === 'chat' ? 'bg-background shadow-xs text-foreground' : 'text-muted-foreground'
            )}
            onclick={() => (activeTab = 'chat')}
          >
            チャット
          </button>
          <button
            type="button"
            class={cn(
              'px-2.5 py-1 text-xs font-medium rounded',
              activeTab === 'diff' ? 'bg-background shadow-xs text-foreground' : 'text-muted-foreground'
            )}
            onclick={() => (activeTab = 'diff')}
          >
            Diff
          </button>
          <button
            type="button"
            class={cn(
              'px-2.5 py-1 text-xs font-medium rounded',
              activeTab === 'terminal' ? 'bg-background shadow-xs text-foreground' : 'text-muted-foreground'
            )}
            onclick={() => (activeTab = 'terminal')}
          >
            Terminal
          </button>
        </div>

        <!-- デスクトップ向け: サイドパネルトグル -->
        <div class="hidden items-center gap-1 md:flex">
          <Button
            variant={sidePanelMode === 'diff' ? 'secondary' : 'outline'}
            size="sm"
            class="h-7 text-xs gap-1.5"
            onclick={() => toggleSidePanel('diff')}
            title="コード変更差分 (Diff) パネルを右側に開閉します"
          >
            <FileCodeIcon class="size-3.5" />
            <span>Diff</span>
          </Button>

          <Button
            variant={sidePanelMode === 'terminal' ? 'secondary' : 'outline'}
            size="sm"
            class="h-7 text-xs gap-1.5"
            onclick={() => toggleSidePanel('terminal')}
            title="ターミナル出力を右側に開閉します"
          >
            <SquareTerminalIcon class="size-3.5" />
            <span>Terminal</span>
          </Button>
        </div>

        <div class="h-4 w-px bg-border/60 mx-0.5 hidden sm:block"></div>

        <!-- Resume ボタン (停止済みセッションの再開) -->
        {#if canResume}
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs gap-1"
            disabled={resuming}
            onclick={handleResumeSession}
            title="停止したセッションを会話コンテキストを維持したまま再開"
          >
            <PlayIcon class="size-3" />
            <span class="hidden sm:inline">Resume</span>
          </Button>
        {/if}

        <!-- Fork ボタン -->
        <Button
          variant="outline"
          size="sm"
          class="h-7 text-xs gap-1"
          onclick={handleForkSession}
          title="このセッションの履歴を引き継いで新しいセッションへ分岐 (Fork)"
        >
          <GitForkIcon class="size-3" />
          <span class="hidden sm:inline">Fork</span>
        </Button>

        <!-- Kill ボタン (強制終了) -->
        <Button
          variant="ghost"
          size="sm"
          class="h-7 text-xs gap-1 text-destructive hover:bg-destructive/10 hover:text-destructive"
          onclick={() => (killDialogOpen = true)}
          title="セッションの子プロセスツリー (Job Object) を即座に強制終了"
        >
          <SkullIcon class="size-3" />
          <span class="hidden sm:inline">Kill</span>
        </Button>
      </div>
    </div>
  </header>

  <!-- Kill 確認モーダル -->
  <AlertDialog.Root bind:open={killDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title class="flex items-center gap-2 text-destructive">
          <SkullIcon class="size-5" />
          セッションを強制終了しますか？
        </AlertDialog.Title>
        <AlertDialog.Description class="text-sm">
          セッション <code>{sessionId}</code> で実行中の子プロセスツリー
          (Job Object) を即座に強制停止します。未コミットの変更や実行中のコマンドは中断されます。
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel disabled={killing}>キャンセル</AlertDialog.Cancel>
        <AlertDialog.Action
          class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
          disabled={killing}
          onclick={handleKillSession}
        >
          {killing ? '強制終了中…' : '強制終了 (Kill)'}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <!-- Revert 確認モーダル (Shadow Git Tree 巻き戻し) -->
  <AlertDialog.Root open={revertSeq !== null} onOpenChange={(open) => !open && (revertSeq = null)}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title class="flex items-center gap-2">
          <Undo2Icon class="size-5 text-amber-500" />
          この時点へ巻き戻しますか？
        </AlertDialog.Title>
        <AlertDialog.Description class="text-sm">
          ワークスペースのファイルを <code>node_seq={revertSeq}</code> のターン開始時の
          スナップショットへ復元します。未コミットの変更は上書きされ、このターン以降に
          作成されたファイルは削除されます。実行直前の状態は Shadow Git Tree にバックアップ
          されます。
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel disabled={reverting}>キャンセル</AlertDialog.Cancel>
        <AlertDialog.Action
          class="bg-amber-600 hover:bg-amber-600/90 text-white"
          disabled={reverting}
          onclick={handleRevertSession}
        >
          {reverting ? '巻き戻し中…' : '巻き戻し (Revert)'}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <!-- セッションが存在しない場合のエラー表示 -->
  {#if session === null}
    <div class="p-4 text-center text-sm text-muted-foreground">
      このセッションは同期されていないか、存在しません。
    </div>
  {/if}

  <!-- メインコンテンツ領域 -->
  <div class="flex flex-1 overflow-hidden">
    <!-- チャットメインエリア (デスクトップ時は常時、モバイル時は activeTab === 'chat' の時に表示) -->
    <div
      class={cn(
        'relative flex h-full flex-1 flex-col overflow-hidden',
        activeTab !== 'chat' ? 'hidden md:flex' : 'flex'
      )}
    >
      <!-- スクロール可能なメッセージタイムライン -->
      <div
        bind:this={chatContainer}
        onscroll={handleScroll}
        class="flex-1 overflow-y-auto px-3 py-4 md:px-6"
      >
        <div class="mx-auto flex max-w-3xl flex-col gap-4">
          <BootstrapLogCard lines={bootstrapLines} />
          <ChatTimeline sessionId={sessionId} {items} onRevert={requestRevert} />
        </div>
      </div>

      <!-- 最下部へジャンプボタン -->
      {#if showScrollBottom}
        <button
          type="button"
          class="bg-primary text-primary-foreground absolute bottom-20 left-1/2 -translate-x-1/2 shadow-lg flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium transition-all hover:opacity-90 animate-in fade-in zoom-in-95"
          onclick={scrollToBottom}
        >
          <ArrowDownIcon class="size-3.5" />
          <span>最新へジャンプ</span>
        </button>
      {/if}

      <!-- チャット入力欄 (Composer) -->
      <div class="bg-card/50 border-t px-3 py-2 md:px-6 shrink-0">
        <div class="mx-auto max-w-3xl">
          <Composer sessionId={sessionId} {capabilities} node={sessionNode} status={currentStatus} />
        </div>
      </div>
    </div>

    <!-- デスクトップ用 右サイドパネル (Diff または Terminal) -->
    {#if sidePanelMode !== null}
      <div class="border-border hidden md:flex md:w-1/2 lg:w-5/12 flex-col border-l bg-card/40 h-full overflow-hidden">
        <!-- サイドパネルヘッダー -->
        <div class="flex items-center justify-between border-b px-3 py-2">
          <div class="flex items-center gap-2 text-xs font-semibold">
            {#if sidePanelMode === 'diff'}
              <FileCodeIcon class="size-4" />
              <span>変更差分 (Diff)</span>
            {:else}
              <SquareTerminalIcon class="size-4" />
              <span>ターミナル (Terminal)</span>
            {/if}
          </div>

          <div class="flex items-center gap-1">
            <button
              type="button"
              class={cn(
                'px-2 py-0.5 text-xs rounded',
                sidePanelMode === 'diff' ? 'bg-secondary font-medium' : 'text-muted-foreground hover:text-foreground'
              )}
              onclick={() => (sidePanelMode = 'diff')}
            >
              Diff
            </button>
            <button
              type="button"
              class={cn(
                'px-2 py-0.5 text-xs rounded',
                sidePanelMode === 'terminal' ? 'bg-secondary font-medium' : 'text-muted-foreground hover:text-foreground'
              )}
              onclick={() => (sidePanelMode = 'terminal')}
            >
              Terminal
            </button>
            <button
              type="button"
              class="text-muted-foreground hover:text-foreground ml-1 p-1"
              onclick={() => (sidePanelMode = null)}
              title="パネルを閉じる"
            >
              <PanelRightCloseIcon class="size-4" />
            </button>
          </div>
        </div>

        <!-- サイドパネルコンテンツ -->
        <div class="flex-1 overflow-y-auto p-3">
          {#if sidePanelMode === 'diff'}
            <DiffPane sessionId={sessionId} {items} />
          {:else if sidePanelMode === 'terminal'}
            <TerminalView {sessionId} />
          {/if}
        </div>
      </div>
    {/if}

    <!-- モバイル用 Diff / Terminal 全画面表示 (md未満でタブ選択時) -->
    {#if activeTab === 'diff'}
      <div class="flex flex-1 flex-col overflow-y-auto p-3 md:hidden">
        <DiffPane sessionId={sessionId} {items} />
      </div>
    {:else if activeTab === 'terminal'}
      <div class="flex flex-1 flex-col overflow-y-auto p-3 md:hidden">
        <TerminalView {sessionId} />
      </div>
    {/if}
  </div>
</div>
