<script lang="ts">
  import type { SessionUsage } from '$lib/generated/SessionUsage';
  import { formatTokenCount } from '$lib/format';

  /**
   * コンテキスト使用量メーター (ACP `usage_update` の投影)。
   *
   * `usage` が `null` (エージェントが usage を報告しない、または未受信) の
   * 場合は何も描画しない。
   */
  let { usage }: { usage: SessionUsage | null } = $props();

  const percent = $derived(
    usage && usage.context_size > 0
      ? Math.min(100, Math.max(0, (usage.used_tokens / usage.context_size) * 100))
      : 0
  );

  const barClass = $derived(
    percent >= 90 ? 'bg-destructive' : percent >= 70 ? 'bg-amber-500' : 'bg-primary'
  );

  const tooltip = $derived(
    usage
      ? `コンテキスト使用量: ${usage.used_tokens} / ${usage.context_size} tokens (${percent.toFixed(1)}%)` +
        (usage.cost ? ` ・累積コスト: ${usage.cost.amount} ${usage.cost.currency}` : '')
      : ''
  );
</script>

{#if usage}
  <span class="inline-flex items-center gap-1.5" title={tooltip}>
    <span class="bg-muted h-1.5 w-16 overflow-hidden rounded-full">
      <span class={`block h-full ${barClass}`} style={`width: ${percent}%`}></span>
    </span>
    <span class="whitespace-nowrap font-mono">
      {formatTokenCount(usage.used_tokens)}/{formatTokenCount(usage.context_size)}
    </span>
    {#if usage.cost}
      <span class="whitespace-nowrap font-mono">
        {usage.cost.currency} {usage.cost.amount.toFixed(4)}
      </span>
    {/if}
  </span>
{/if}
