<script lang="ts">
  import type { SessionUsage } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { formatTokenCount } from "#lib/format.ts";

  /**
   * Context usage meter (a projection of ACP `usage_update`).
   *
   * Renders nothing when `usage` is `null` (the agent does not report usage,
   * or nothing has been received yet).
   */
  let { usage }: { usage: SessionUsage | null } = $props();

  const percent = $derived(
    usage && usage.context_size > 0
      ? Math.min(100, Math.max(0, (usage.used_tokens / usage.context_size) * 100))
      : 0,
  );

  const barClass = $derived(percent >= 90 ? "bg-danger" : percent >= 70 ? "bg-warn" : "bg-signal");

  const tooltip = $derived(
    usage
      ? m.session_context_usage({
          used: usage.used_tokens,
          total: usage.context_size,
          percent: percent.toFixed(1),
        }) +
          (usage.cost
            ? ` — ${m.session_context_cost({
                amount: usage.cost.amount,
                currency: usage.cost.currency,
              })}`
            : "")
      : "",
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
        {usage.cost.currency}
        {usage.cost.amount.toFixed(4)}
      </span>
    {/if}
  </span>
{/if}
