<script lang="ts">
  import PermissionCard from '$lib/components/PermissionCard.svelte';
  import ToolCallCard from './ToolCallCard.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { formatEpochMs } from '$lib/format';
  import type { TimelineItem } from '$lib/sync/reducer';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
  import BotIcon from '@lucide/svelte/icons/bot';
  import BrainIcon from '@lucide/svelte/icons/brain';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import ListChecksIcon from '@lucide/svelte/icons/list-checks';
  import SquareTerminalIcon from '@lucide/svelte/icons/square-terminal';

  let { sessionId, items }: { sessionId: string; items: TimelineItem[] } = $props();

  function planStatusClass(status: string): string {
    switch (status) {
      case 'completed':
        return 'text-muted-foreground line-through';
      case 'in_progress':
        return 'font-medium';
      default:
        return 'text-muted-foreground';
    }
  }
</script>

<ul class="flex list-none flex-col gap-3 p-0">
  {#each items as item (item.key)}
    <li>
      {#if item.kind === 'user'}
        <div class="flex justify-end">
          <div class="bg-primary text-primary-foreground max-w-[85%] rounded-2xl px-3.5 py-2">
            <p class="text-sm whitespace-pre-wrap">{item.text}</p>
            <p class="text-primary-foreground/70 mt-1 text-right text-[10px]">
              {item.clientSource} · {formatEpochMs(item.createdAt)}
            </p>
          </div>
        </div>
      {:else if item.kind === 'agent'}
        <div class="flex gap-2">
          <BotIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <div class="max-w-[92%] flex-1">
            <p class="text-sm leading-relaxed whitespace-pre-wrap">
              {item.text}{#if item.streaming}<span class="animate-pulse">▍</span>{/if}
            </p>
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
          <p
            class="text-muted-foreground border-muted-foreground/30 mt-1.5 border-l-2 pl-3 text-xs leading-relaxed whitespace-pre-wrap"
          >
            {item.text}{#if item.streaming}<span class="animate-pulse">▍</span>{/if}
          </p>
        </details>
      {:else if item.kind === 'tool'}
        <ToolCallCard {item} />
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
        <div class="text-muted-foreground flex items-center gap-2 text-sm">
          <ClockIcon class="size-4 animate-pulse" />
          <span>送信待ち: <span class="whitespace-pre-wrap">{item.text}</span></span>
        </div>
      {/if}
    </li>
  {/each}
</ul>

{#if items.length === 0}
  <p class="text-muted-foreground text-sm">まだイベントがありません。</p>
{/if}
