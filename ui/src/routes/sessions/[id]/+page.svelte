<script lang="ts">
  import { page } from '$app/state';
  import BootstrapLogCard from '$lib/components/BootstrapLogCard.svelte';
  import ChatTimeline from '$lib/components/chat/ChatTimeline.svelte';
  import Composer from '$lib/components/chat/Composer.svelte';
  import DiffPane from '$lib/components/diff/DiffPane.svelte';
  import SessionStatusBadge from '$lib/components/SessionStatusBadge.svelte';
  import TerminalView from '$lib/components/terminal/TerminalView.svelte';
  import { Tabs, TabsContent, TabsList, TabsTrigger } from '$lib/components/ui/tabs';
  import { formatRelativeTime } from '$lib/format';
  import { shortId } from '$lib/session-status';
  import { sync } from '$lib/stores/app.svelte';
  import { bootstrapLogLines, buildTimelineItems, capabilitiesFromEvents } from '$lib/sync/reducer';

  const sessionId = $derived(page.params.id ?? '');
  const session = $derived(sync.sessions.find((entry) => entry.session_id === sessionId) ?? null);
  const timeline = $derived(sync.timelineFor(sessionId));
  // 派生値はコンポーネント側で構成する (ストア内の $derived は derived_inert
  // の原因になるため)
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
  const bootstrapLines = $derived(bootstrapLogLines(timeline.events));
  const nodeName = $derived(
    session === null
      ? ''
      : (sync.nodes.find((node) => node.node_id === session.node_id)?.name ??
        shortId(session.node_id))
  );

  let chatContainer = $state<HTMLDivElement | null>(null);
  /** 履歴 REST ロードを実行したセッションID (1回だけ実行する)。 */
  let loadedFor = $state<string | null>(null);

  // セッションを WS の優先配信対象にし、履歴未取得なら REST でフォールバック読込する
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

  // 新規アイテム追加時に末尾へスクロール
  $effect(() => {
    const currentItems = items;
    if (chatContainer !== null && currentItems.length > 0) {
      chatContainer.scrollTop = chatContainer.scrollHeight;
    }
  });
</script>

<div class="flex flex-col gap-3">
  <div class="bg-card flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border p-3">
    <div class="flex min-w-0 flex-1 flex-col gap-0.5">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="truncate text-lg font-semibold">
          {session?.title ?? 'セッション'}
        </h1>
        {#if session}
          <SessionStatusBadge status={session.status} />
        {/if}
      </div>
      {#if session}
        <div class="text-muted-foreground flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
          <span>{nodeName}</span>
          <span class="truncate">{session.project_id}</span>
          {#if session.git_branch}
            <span>branch: {session.git_branch}</span>
          {/if}
          {#if session.is_worktree}
            <span>worktree</span>
          {/if}
          <span>{session.agent_id}</span>
          {#if session.parent_session_id}
            <span>fork of {shortId(session.parent_session_id)}</span>
          {/if}
          <span>更新: {formatRelativeTime(session.updated_at)}</span>
        </div>
      {/if}
    </div>
  </div>

  {#if session === null}
    <p class="text-muted-foreground text-sm">
      このセッションはまだイベントが同期されていないか、存在しません。
    </p>
  {/if}

  <Tabs value="chat">
    <TabsList>
      <TabsTrigger value="chat">チャット</TabsTrigger>
      <TabsTrigger value="diff">Diff</TabsTrigger>
      <TabsTrigger value="terminal">Terminal</TabsTrigger>
    </TabsList>

    <TabsContent value="chat" class="flex flex-col gap-3">
      <BootstrapLogCard lines={bootstrapLines} />
      <div
        bind:this={chatContainer}
        class="bg-background/60 max-h-[62svh] overflow-y-auto rounded-lg border p-3"
      >
        <ChatTimeline sessionId={sessionId} {items} />
      </div>
      <Composer sessionId={sessionId} {capabilities} />
    </TabsContent>

    <TabsContent value="diff">
      <DiffPane sessionId={sessionId} {items} />
    </TabsContent>

    <TabsContent value="terminal">
      <TerminalView {sessionId} />
    </TabsContent>
  </Tabs>
</div>
