import type { SessionUsage } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { formatTokenCount } from "#lib/format.ts";
import { Show } from "solid-js";

export interface ContextMeterProps {
  usage: SessionUsage | null;
}

/**
 * Context usage meter (a projection of ACP `usage_update`).
 *
 * Renders nothing when `usage` is `null` (the agent does not report usage, or
 * nothing has been received yet).
 */
export default function ContextMeter(props: ContextMeterProps) {
  const percent = () => {
    const usage = props.usage;
    return usage !== null && usage.context_size > 0
      ? Math.min(100, Math.max(0, (usage.used_tokens / usage.context_size) * 100))
      : 0;
  };

  const barClass = () =>
    percent() >= 90 ? "bg-danger" : percent() >= 70 ? "bg-warn" : "bg-signal";

  const tooltip = () => {
    const usage = props.usage;
    if (usage === null) return "";
    return (
      m.session_context_usage({
        used: usage.used_tokens,
        total: usage.context_size,
        percent: percent().toFixed(1),
      }) +
      (usage.cost
        ? ` — ${m.session_context_cost({
            amount: usage.cost.amount,
            currency: usage.cost.currency,
          })}`
        : "")
    );
  };

  return (
    <Show when={props.usage}>
      {(usage) => (
        <span class="inline-flex items-center gap-1.5" title={tooltip()}>
          <span class="bg-muted h-1.5 w-16 overflow-hidden rounded-full">
            <span class={`block h-full ${barClass()}`} style={{ width: `${percent()}%` }} />
          </span>
          <span class="font-mono whitespace-nowrap">
            {formatTokenCount(usage().used_tokens)} / {formatTokenCount(usage().context_size)}
          </span>
          <Show when={usage().cost}>
            {(cost) => (
              <Show when={cost().amount > 0}>
                <span class="font-mono whitespace-nowrap">
                  {cost().currency}
                  {cost().amount.toFixed(4)}
                </span>
              </Show>
            )}
          </Show>
        </span>
      )}
    </Show>
  );
}
