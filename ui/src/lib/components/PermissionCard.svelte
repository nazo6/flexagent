<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import type { PermissionOption } from '$lib/generated/PermissionOption';
  import { formatRelativeTime } from '$lib/format';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import CheckIcon from '@lucide/svelte/icons/check';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
  import XIcon from '@lucide/svelte/icons/x';

  let {
    sessionId,
    requestId,
    toolName,
    summary,
    options,
    details = null,
    resolved = null,
    createdAt = null
  }: {
    sessionId: string;
    requestId: string;
    toolName: string;
    summary: string;
    options: PermissionOption[];
    details?: unknown;
    resolved?: { selectedOptionId: string; resolvedBy: string } | null;
    createdAt?: number | null;
  } = $props();

  let busyOptionId = $state<string | null>(null);

  const detailsJson = $derived(
    details === null || details === undefined ? null : JSON.stringify(details, null, 2)
  );

  function isAllowOption(kind: string): boolean {
    return kind.startsWith('allow');
  }

  async function respond(option: PermissionOption) {
    if (busyOptionId !== null) return;
    busyOptionId = option.option_id;
    try {
      const result = await sync.respondPermission(sessionId, requestId, option.option_id);
      if (result.code === 'ALREADY_RESOLVED') {
        toast.info('既に他のクライアントで解決済みでした');
      } else if (!result.success) {
        toast.error(result.error ?? result.code ?? '承認応答に失敗しました');
      } else {
        toast.success('応答を送信しました');
      }
      await sync.refreshInbox();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyOptionId = null;
    }
  }
</script>

<div class="bg-card flex flex-col gap-3 rounded-lg border p-3">
  <div class="flex flex-wrap items-start justify-between gap-2">
    <div class="flex min-w-0 flex-col gap-1">
      <div class="flex items-center gap-2">
        <ShieldCheckIcon class="text-muted-foreground size-4 shrink-0" />
        <span class="font-mono text-xs font-medium">{toolName}</span>
        {#if resolved}
          <Badge variant="secondary">
            {resolved.selectedOptionId} ({resolved.resolvedBy})
          </Badge>
        {:else if createdAt !== null}
          <span class="text-muted-foreground text-xs">{formatRelativeTime(createdAt)}</span>
        {/if}
      </div>
      <p class="text-sm break-words">{summary}</p>
    </div>
    <a
      href={`/sessions/${sessionId}`}
      class="text-muted-foreground hover:text-foreground flex shrink-0 items-center gap-0.5 text-xs"
    >
      セッションを開く
      <ChevronRightIcon class="size-3" />
    </a>
  </div>

  {#if detailsJson !== null}
    <details class="group">
      <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-xs select-none">
        詳細 (コマンド / Diff)
      </summary>
      <pre
        class="bg-muted/50 mt-2 max-h-56 overflow-auto rounded-md p-2 font-mono text-xs whitespace-pre-wrap">{detailsJson}</pre>
    </details>
  {/if}

  {#if resolved === null}
    <div class="flex flex-wrap gap-2">
      {#each options as option (option.option_id)}
        <Button
          size="sm"
          variant={isAllowOption(option.kind) ? 'default' : 'outline'}
          disabled={busyOptionId !== null}
          onclick={() => void respond(option)}
        >
          {#if isAllowOption(option.kind)}
            <CheckIcon />
          {:else}
            <XIcon />
          {/if}
          {option.name || option.option_id}
        </Button>
      {/each}
    </div>
  {/if}
</div>
