<script lang="ts">
  import DiffText from '$lib/components/diff/DiffText.svelte';
  import type { TimelineItem } from '$lib/sync/reducer';
  import { cn } from '$lib/utils';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
  import CircleXIcon from '@lucide/svelte/icons/circle-x';
  import FileSearchIcon from '@lucide/svelte/icons/file-search';
  import HammerIcon from '@lucide/svelte/icons/hammer';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import PencilIcon from '@lucide/svelte/icons/pencil';
  import SearchIcon from '@lucide/svelte/icons/search';
  import SquareTerminalIcon from '@lucide/svelte/icons/square-terminal';
  import WrenchIcon from '@lucide/svelte/icons/wrench';

  type ToolItem = Extract<TimelineItem, { kind: 'tool' }>;

  interface Props {
    tools: ToolItem[];
  }

  let { tools }: Props = $props();

  let isOpen = $state(true);
  let expandedDetails = $state<Record<string, boolean>>({});

  function toggleDetail(toolCallId: string, event: MouseEvent) {
    event.stopPropagation();
    expandedDetails[toolCallId] = !expandedDetails[toolCallId];
  }

  const hasInProgress = $derived(tools.some((t) => t.status === 'in_progress'));
  const hasFailed = $derived(tools.some((t) => t.status === 'failed'));
  const allCompleted = $derived(tools.every((t) => t.status === 'completed'));

  function getToolInfo(tool: ToolItem): { name: string; detail: string } {
    const kindMap: Record<string, string> = {
      read: 'Read',
      edit: 'Edit',
      execute: 'Terminal',
      search: 'Search',
      other: 'Tool'
    };

    const colonIdx = tool.title.indexOf(':');
    if (colonIdx > 0 && colonIdx < 30) {
      const rawName = tool.title.slice(0, colonIdx).trim();
      const detail = tool.title.slice(colonIdx + 1).trim();
      return {
        name: rawName,
        detail: detail !== '' ? detail : rawName
      };
    }

    const fallbackName = kindMap[tool.toolKind] ?? 'Tool';
    return {
      name: fallbackName,
      detail: tool.title !== '' ? tool.title : fallbackName
    };
  }
</script>

<div class="bg-card/70 border-border/70 overflow-hidden rounded-xl border text-xs shadow-2xs transition-colors">
  <!-- グループヘッダー -->
  <button
    type="button"
    class="hover:bg-accent/40 flex w-full items-center justify-between px-3 py-2 text-left font-medium select-none"
    onclick={() => (isOpen = !isOpen)}
  >
    <div class="flex items-center gap-2">
      <div class="text-muted-foreground flex items-center gap-1.5">
        {#if isOpen}
          <ChevronDownIcon class="size-3.5" />
        {:else}
          <ChevronRightIcon class="size-3.5" />
        {/if}
        <WrenchIcon class="size-3.5" />
      </div>
      <span class="font-medium text-foreground">
        Tool calls · {tools.length}
      </span>
    </div>

    <div class="flex items-center gap-2">
      {#if hasInProgress}
        <span class="flex items-center gap-1 text-primary text-[11px]">
          <LoaderCircleIcon class="size-3.5 animate-spin" />
          <span>実行中…</span>
        </span>
      {:else if hasFailed}
        <span class="flex items-center gap-1 text-destructive text-[11px]">
          <CircleXIcon class="size-3.5" />
          <span>エラー</span>
        </span>
      {:else if allCompleted}
        <CircleCheckIcon class="size-4 text-emerald-500" />
      {/if}
    </div>
  </button>

  <!-- ツール呼び出し一覧 (展開時) -->
  {#if isOpen}
    <div class="border-border/60 divide-border/60 divide-y border-t bg-muted/20">
      {#each tools as tool (tool.toolCallId)}
        {@const isExpanded = expandedDetails[tool.toolCallId]}
        {@const toolInfo = getToolInfo(tool)}
        {@const hasDetailContent = tool.diff !== null || (tool.rawOutput !== null && tool.rawOutput !== '') || tool.locations.length > 0}
        <div class="flex flex-col">
          <!-- ツール単行サマリー -->
          <div
            role="button"
            tabindex="0"
            class={cn(
              'flex items-center justify-between px-3 py-1.5 text-xs transition-colors',
              hasDetailContent ? 'cursor-pointer hover:bg-accent/30' : 'cursor-default'
            )}
            onclick={(e) => {
              if (hasDetailContent) toggleDetail(tool.toolCallId, e);
            }}
            onkeydown={(e) => {
              if (hasDetailContent && (e.key === 'Enter' || e.key === ' ')) {
                e.preventDefault();
                expandedDetails[tool.toolCallId] = !expandedDetails[tool.toolCallId];
              }
            }}
          >
            <div class="flex min-w-0 flex-1 items-center gap-2">
              <!-- アイコン -->
              {#if tool.toolKind === 'read'}
                <FileSearchIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === 'edit'}
                <PencilIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === 'execute'}
                <SquareTerminalIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === 'search'}
                <SearchIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else}
                <HammerIcon class="text-muted-foreground size-3.5 shrink-0" />
              {/if}

              <!-- ツール名バッジ (ツール名を明示) -->
              <span class="rounded bg-muted px-1.5 py-0.2 font-mono text-[10px] font-semibold text-foreground/85 shrink-0 border border-border/60">
                {toolInfo.name}
              </span>

              <!-- コマンド / 引数 / タイトル詳細 -->
              <span class="truncate font-mono text-[11px] text-foreground/90">
                {toolInfo.detail}
              </span>

              {#if tool.locations.length > 0}
                <span class="text-muted-foreground truncate font-mono text-[10px]">
                  ({tool.locations.join(', ')})
                </span>
              {/if}
            </div>

            <!-- ステータス & 展開トグル -->
            <div class="ml-2 flex shrink-0 items-center gap-2">
              {#if tool.status === 'in_progress'}
                <LoaderCircleIcon class="size-3 animate-spin text-primary" />
              {:else if tool.status === 'completed'}
                <CircleCheckIcon class="size-3.5 text-emerald-500" />
              {:else if tool.status === 'failed'}
                <CircleXIcon class="size-3.5 text-destructive" />
              {/if}

              {#if hasDetailContent}
                <span class="text-muted-foreground text-[10px]">
                  {isExpanded ? '▲' : '▼'}
                </span>
              {/if}
            </div>
          </div>

          <!-- 詳細コンテンツ (差分 / ログ出力) -->
          {#if isExpanded && hasDetailContent}
            <div class="bg-background/80 border-border/60 border-t p-2.5 flex flex-col gap-2">
              <div class="flex items-center gap-2 text-[11px]">
                <span class="text-muted-foreground text-[10px]">ツール:</span>
                <span class="font-mono font-semibold text-foreground">{toolInfo.name}</span>
                <span class="text-muted-foreground text-[10px]">種別:</span>
                <span class="font-mono text-muted-foreground">{tool.toolKind}</span>
                {#if tool.status}
                  <span class="text-muted-foreground text-[10px]">状態:</span>
                  <span class="font-mono text-muted-foreground">{tool.status}</span>
                {/if}
              </div>

              {#if tool.locations.length > 0}
                <div class="flex flex-col gap-0.5 text-[11px]">
                  <span class="text-muted-foreground text-[10px]">対象ファイル:</span>
                  <ul class="flex flex-col gap-0.5 font-mono text-muted-foreground p-0 list-none">
                    {#each tool.locations as loc (loc)}
                      <li class="truncate">{loc}</li>
                    {/each}
                  </ul>
                </div>
              {/if}

              {#if tool.diff !== null}
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[10px]">Diff:</span>
                  <DiffText diff={tool.diff.unified_diff} maxHeight="12rem" />
                </div>
              {/if}

              {#if tool.rawOutput !== null && tool.rawOutput !== ''}
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[10px]">出力:</span>
                  <pre class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{tool.rawOutput}</pre>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
