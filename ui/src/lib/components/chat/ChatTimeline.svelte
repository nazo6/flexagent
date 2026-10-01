<script lang="ts">
  import { goto } from '$app/navigation';
  import PermissionCard from '$lib/components/PermissionCard.svelte';
  import MarkdownText from '$lib/components/MarkdownText.svelte';
  import ToolCallGroup from './ToolCallGroup.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { formatEpochMs } from '$lib/format';
  import { sync } from '$lib/stores/app.svelte';
  import type { SessionStatus } from '$lib/generated/SessionStatus';
  import type { TimelineItem } from '$lib/sync/reducer';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
  import BotIcon from '@lucide/svelte/icons/bot';
  import BrainIcon from '@lucide/svelte/icons/brain';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import GitForkIcon from '@lucide/svelte/icons/git-fork';
  import ListChecksIcon from '@lucide/svelte/icons/list-checks';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import SquareTerminalIcon from '@lucide/svelte/icons/square-terminal';
  import Undo2Icon from '@lucide/svelte/icons/undo-2';

  let {
    sessionId,
    items,
    status = null,
    onRevert
  }: {
    sessionId: string;
    items: TimelineItem[];
    /** セッション状態 (イベント由来)。エージェント実行中のインジケータ表示に使う。 */
    status?: SessionStatus | null;
    /** 「この時点へ巻き戻す (Revert)」要求のコールバック (引数は対象ターンの node_seq)。 */
    onRevert?: (seq: number) => void;
  } = $props();

  const currentSession = $derived(
    sync.sessions.find((s) => s.session_id === sessionId) ?? null
  );

  /** 停止済みセッションは Revert 不可 (ノード側で実行中セッションのみ許容)。 */
  const canRevert = $derived(
    onRevert !== undefined && currentSession !== null && currentSession.status !== 'stopped'
  );

  /**
   * エージェントがターンを実行中 (応答生成・ツール実行中)。
   *
   * 送信直後〜最初の出力到着までだけでなく、ターンが終わる (idle / stopped /
   * error) までタイムライン末尾に処理中インジケータを表示する。承認待ち
   * (`waiting_permission`) はユーザー操作待ちのため表示しない。
   */
  const agentWorking = $derived(status === 'running');

  function forkFromSeq(seq: number) {
    const params = new URLSearchParams();
    params.set('fork_session', sessionId);
    params.set('fork_seq', String(seq));
    if (currentSession?.project_id) params.set('project', currentSession.project_id);
    if (currentSession?.agent_id) params.set('agent', currentSession.agent_id);
    void goto(`/?${params.toString()}`);
  }

  function planStatusClass(planStatus: string): string {
    switch (planStatus) {
      case 'completed':
        return 'text-muted-foreground line-through';
      case 'in_progress':
        return 'font-medium';
      default:
        return 'text-muted-foreground';
    }
  }

  type RenderGroup =
    | { kind: 'single'; key: string; item: TimelineItem }
    | { kind: 'tools'; key: string; tools: Extract<TimelineItem, { kind: 'tool' }>[] };

  const groupedItems = $derived.by(() => {
    const groups: RenderGroup[] = [];
    let currentTools: Extract<TimelineItem, { kind: 'tool' }>[] = [];

    for (const item of items) {
      if (item.kind === 'tool') {
        currentTools.push(item);
      } else {
        if (currentTools.length > 0) {
          groups.push({
            kind: 'tools',
            key: `toolgroup:${currentTools[0].toolCallId}`,
            tools: currentTools
          });
          currentTools = [];
        }
        groups.push({ kind: 'single', key: item.key, item });
      }
    }

    if (currentTools.length > 0) {
      groups.push({
        kind: 'tools',
        key: `toolgroup:${currentTools[0].toolCallId}`,
        tools: currentTools
      });
    }

    return groups;
  });
</script>

<ul class="flex list-none flex-col gap-3 p-0">
  {#each groupedItems as group (group.key)}
    <li>
      {#if group.kind === 'tools'}
        <ToolCallGroup tools={group.tools} />
      {:else}
        {@const item = group.item}
        {#if item.kind === 'user'}
          <div class="group flex items-end justify-end gap-1.5">
            {#if canRevert}
              <button
                type="button"
                class="hover:bg-accent text-muted-foreground hover:text-foreground opacity-0 group-hover:opacity-100 mb-1 flex size-6 items-center justify-center rounded border transition-opacity cursor-pointer"
                title="この時点へ巻き戻す (Revert: ワークスペースのファイルをこのターン開始時の状態へ復元)"
                onclick={() => onRevert?.(item.seq)}
              >
                <Undo2Icon class="size-3" />
              </button>
            {/if}
            <button
              type="button"
              class="hover:bg-accent text-muted-foreground hover:text-foreground opacity-0 group-hover:opacity-100 mb-1 flex size-6 items-center justify-center rounded border transition-opacity cursor-pointer"
              title="このターンからフォーク (新規セッションを開始)"
              onclick={() => forkFromSeq(item.seq)}
            >
              <GitForkIcon class="size-3" />
            </button>
            <div class="bg-primary text-primary-foreground max-w-[85%] rounded-2xl px-3.5 py-2">
              <p class="text-sm whitespace-pre-wrap">{item.text}</p>
              <p class="text-primary-foreground/70 mt-1 text-right text-[10px]">
                {item.clientSource} · {formatEpochMs(item.createdAt)}
              </p>
            </div>
          </div>
        {:else if item.kind === 'agent'}
          <div class="group flex gap-2">
            <BotIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
            <div class="max-w-[92%] flex-1">
              <MarkdownText
                text={item.text}
                streaming={item.streaming}
                class="text-sm leading-relaxed"
              />
              {#if !item.streaming}
                <div class="mt-1 flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                  <button
                    type="button"
                    class="hover:bg-accent text-muted-foreground hover:text-foreground flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] border transition-colors cursor-pointer"
                    title="この回答時点からフォーク (新規セッションを開始)"
                    onclick={() => forkFromSeq(item.seq)}
                  >
                    <GitForkIcon class="size-3" />
                    <span>ここからフォーク</span>
                  </button>
                </div>
              {/if}
            </div>
          </div>
        {:else if item.kind === 'thought'}
          <details class="group">
            <summary
              class="text-muted-foreground hover:text-foreground flex cursor-pointer items-center gap-1.5 text-xs select-none"
            >
              <BrainIcon class="size-3.5" />
              思考プロセス
              {#if item.streaming}<span class="animate-pulse">…</span>{/if}
            </summary>
            <div
              class="text-muted-foreground border-muted-foreground/30 mt-1.5 border-l-2 pl-3 text-xs leading-relaxed"
            >
              <MarkdownText text={item.text} streaming={item.streaming} />
            </div>
          </details>
      {:else if item.kind === 'terminal'}
        <div class="bg-card/60 flex flex-col gap-1.5 rounded-lg border p-2.5">
          <div class="flex items-center gap-2 text-xs">
            <SquareTerminalIcon class="text-muted-foreground size-3.5" />
            <span class="font-mono">{item.command === '' ? item.terminalId : item.command}</span>
            {#if item.exitCode !== null}
              <Badge variant={item.exitCode === 0 ? 'secondary' : 'destructive'}>
                exit {item.exitCode}
              </Badge>
            {/if}
          </div>
          <pre
            class="bg-muted/30 max-h-56 overflow-auto rounded-md border p-2 font-mono text-xs whitespace-pre-wrap">{item.output}</pre>
        </div>
      {:else if item.kind === 'permission'}
        <PermissionCard
          sessionId={sessionId}
          requestId={item.requestId}
          toolName={item.toolName}
          summary={item.summary}
          options={item.options}
          details={item.details}
          resolved={item.resolved}
          createdAt={item.createdAt}
        />
      {:else if item.kind === 'plan'}
        <div class="bg-card/60 flex flex-col gap-1.5 rounded-lg border p-2.5">
          <div class="flex items-center gap-2 text-xs font-medium">
            <ListChecksIcon class="size-3.5" />
            実行計画
          </div>
          <ul class="flex list-none flex-col gap-1 p-0 text-sm">
            {#each item.entries as entry (entry.id)}
              <li class={planStatusClass(entry.status)}>{entry.title}</li>
            {/each}
          </ul>
        </div>
      {:else if item.kind === 'notice'}
        <div
          class="flex items-start gap-2 rounded-lg border p-2.5 text-sm"
          class:text-destructive={item.tone === 'error'}
        >
          {#if item.tone === 'error'}
            <AlertTriangleIcon class="mt-0.5 size-4 shrink-0" />
          {:else}
            <ClockIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
          {/if}
          <p class="whitespace-pre-wrap">{item.text}</p>
        </div>
      {:else if item.kind === 'pending'}
        <div class="text-muted-foreground flex items-center gap-2 text-sm" role="status">
          <LoaderCircleIcon class="size-4 animate-spin text-primary" />
          <span>送信待ち: <span class="whitespace-pre-wrap">{item.text}</span></span>
        </div>
      {/if}
    {/if}
  </li>
{/each}

{#if agentWorking}
  <li class="flex gap-2" role="status" aria-live="polite">
    <BotIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
    <div
      class="bg-muted/60 text-muted-foreground flex items-center gap-2 rounded-2xl px-3.5 py-2 text-sm"
    >
      <LoaderCircleIcon class="size-3.5 animate-spin text-primary" />
      <span>エージェントが処理中…</span>
    </div>
  </li>
{/if}
</ul>

{#if items.length === 0 && !agentWorking}
  <p class="text-muted-foreground text-sm">
    まだ表示できるイベントがありません。プロンプトを送信するとここに表示されます。
  </p>
{/if}
