<script lang="ts">
  import { untrack } from "svelte";
  import AgentIcon from "#lib/components/agent/AgentIcon.svelte";
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import type { AgentUsageLimits, UsageGroup, UsageWindow } from "#lib/generated/bindings.ts";
  import { formatDurationMs, formatRelativeTime } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  interface Props {
    /** Node that owns the agent process. */
    nodeId: string;
    /** Agent ID of the session. */
    agentId: string;
    /** Working directory used for agent-specific lookups. */
    localPath: string;
    class?: string;
  }

  let { nodeId, agentId, localPath, class: className = "" }: Props = $props();

  const REFRESH_MS = 60_000;

  let usage = $state<AgentUsageLimits | null>(null);
  let loading = $state(false);
  let requestId = 0;

  /**
   * Usage limits are an optional, out-of-session probe. A failure (agent not
   * running, no login, unsupported build) hides the meter instead of surfacing
   * a session error.
   */
  async function load(node = nodeId, agent = agentId, path = localPath): Promise<void> {
    if (node === "" || agent === "") return;
    const current = ++requestId;
    loading = true;
    try {
      const response = await sync.connection.client.agentUsage(node, agent, path);
      if (current === requestId) usage = response.usage;
    } catch {
      // Keep the last snapshot; the next refresh may succeed.
    } finally {
      if (current === requestId) loading = false;
    }
  }

  $effect(() => {
    const node = nodeId;
    const agent = agentId;
    const path = localPath;
    if (node === "" || agent === "") return;
    untrack(() => void load(node, agent, path));
    const timer = setInterval(() => void load(node, agent, path), REFRESH_MS);
    return () => clearInterval(timer);
  });

  /** Flattened window with the least remaining quota (used for the chip). */
  interface WorstWindow {
    group: UsageGroup;
    window: UsageWindow;
    remaining: number;
  }

  const worst = $derived.by((): WorstWindow | null => {
    if (usage === null) return null;
    let result: WorstWindow | null = null;
    for (const group of usage.groups) {
      for (const window of group.windows) {
        const remaining = Math.max(0, Math.min(1, 1 - window.used_fraction));
        if (window.limited) return { group, window, remaining: 0 };
        if (result === null || remaining < result.remaining) {
          result = { group, window, remaining };
        }
      }
    }
    return result;
  });

  function remainingPercent(window: UsageWindow): number {
    return Math.round(Math.max(0, Math.min(1, 1 - window.used_fraction)) * 100);
  }

  function windowLabel(window: UsageWindow): string {
    switch (window.kind) {
      case "rolling_5h":
        return m.session_usage_window_rolling_5h();
      case "5h":
        return m.session_usage_window_5h();
      case "weekly":
        return m.session_usage_window_weekly();
      case "monthly":
        return m.session_usage_window_monthly();
      case "budget":
        return m.session_usage_window_budget();
      default:
        return window.label ?? m.session_usage_window_other();
    }
  }

  /** Compact window label for the phone chip (full label is in the popover). */
  function compactWindowLabel(window: UsageWindow): string {
    switch (window.kind) {
      case "rolling_5h":
      case "5h":
        return "5h";
      case "weekly":
        return "1w";
      case "monthly":
        return "1m";
      case "budget":
        return m.session_usage_window_budget();
      default:
        return window.label ?? m.session_usage_window_other();
    }
  }

  function barClass(window: UsageWindow): string {
    const remaining = remainingPercent(window);
    if (window.limited || remaining <= 10) return "bg-danger";
    if (remaining <= 35) return "bg-warn";
    return "bg-signal";
  }

  function resetsIn(window: UsageWindow): string | null {
    if (window.resets_at === null) return null;
    const remaining = window.resets_at - Date.now();
    return remaining > 0 ? formatDurationMs(remaining) : null;
  }

  const chipLabel = $derived.by(() => {
    if (usage === null || worst === null) return "";
    const group = usage.groups.length > 1 ? `${worst.group.label} · ` : "";
    if (worst.window.limited) {
      return `${usage.source.label} · ${group}${m.session_usage_limited()}`;
    }
    return `${usage.source.label} · ${group}${windowLabel(worst.window)} ${m.session_usage_left({
      percent: remainingPercent(worst.window),
    })}`;
  });
</script>

{#if usage}
  <Popover.Root>
    <Popover.Trigger
      class={cn(
        "text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/40 inline-flex max-w-full min-w-0 shrink items-center gap-1.5 rounded-md border border-transparent px-1.5 py-1 text-[11px] outline-none transition-colors focus-visible:ring-2 sm:max-w-[18rem]",
        className,
      )}
      title={m.session_usage_title()}
      aria-label={chipLabel}
    >
      <AgentIcon icon={usage.source.icon} class="size-3.5" />
      <span class="min-w-0 truncate">
        {usage.source.label}
        {#if worst}
          {#if usage.groups.length > 1}
            <span class="hidden md:inline"> · {worst.group.label}</span>
          {/if}
          <span>
            ·
            {#if worst.window.limited}
              {m.session_usage_limited()}
            {:else}
              <span class="hidden sm:inline">{windowLabel(worst.window)}</span>
              <span class="sm:hidden">{compactWindowLabel(worst.window)}</span>
              {m.session_usage_left({ percent: remainingPercent(worst.window) })}
            {/if}
          </span>
        {/if}
      </span>
      {#if worst?.window.limited}
        <span class="bg-danger size-1.5 shrink-0 rounded-full"></span>
      {/if}
    </Popover.Trigger>

    <Popover.Content align="start" class="w-[min(23rem,calc(100vw-2rem))] p-0">
      <div class="border-b border-border px-3 py-2">
        <div class="flex min-w-0 items-center gap-2">
          <AgentIcon icon={usage.source.icon} class="size-4" />
          <div class="min-w-0">
            <div class="flex min-w-0 items-center gap-1.5">
              <h2 class="truncate text-[13px] font-semibold">{usage.source.label}</h2>
              {#if usage.source.plan}
                <span class="bg-muted text-muted-foreground rounded px-1.5 py-0.5 text-[10px]">
                  {usage.source.plan}
                </span>
              {/if}
            </div>
            <div class="text-muted-foreground truncate text-[11px]">
              {usage.source.id}{usage.source.account ? ` · ${usage.source.account}` : ""}
            </div>
          </div>
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground hover:bg-accent ml-auto flex size-6 shrink-0 items-center justify-center rounded-md transition-colors"
            title={m.session_usage_refresh()}
            aria-label={m.session_usage_refresh()}
            onclick={() => void load()}
            disabled={loading}
          >
            <RefreshCwIcon class={cn("size-3.5", loading && "animate-spin")} />
          </button>
        </div>
      </div>

      <div class="space-y-3 p-3">
        {#each usage.groups as group (group.id)}
          <section>
            <div class="flex items-baseline justify-between gap-2">
              <h3 class="text-[12px] font-semibold">{group.label}</h3>
            </div>
            {#if group.description}
              <p class="text-muted-foreground mt-0.5 text-[10.5px] leading-snug">
                {group.description}
              </p>
            {/if}
            <div class="mt-1.5 space-y-1.5">
              {#each group.windows as window (window.kind + (window.label ?? ""))}
                {@const reset = resetsIn(window)}
                <div class="grid grid-cols-[3.4rem_minmax(3rem,1fr)_4.5rem] items-center gap-2">
                  <span class="text-[11px] font-medium">{windowLabel(window)}</span>
                  <span class="bg-muted h-1.5 overflow-hidden rounded-full">
                    <span
                      class={cn("block h-full rounded-full", barClass(window))}
                      style={`width: ${remainingPercent(window)}%`}
                    ></span>
                  </span>
                  <span class="text-right font-mono text-[10.5px] tabular-nums">
                    {#if window.limited}
                      <span class="text-destructive">{m.session_usage_limited()}</span>
                    {:else}
                      {m.session_usage_left({ percent: remainingPercent(window) })}
                    {/if}
                  </span>
                </div>
                {#if reset !== null}
                  <div class="text-muted-foreground -mt-1 text-right text-[10px]">
                    {m.session_usage_resets_in({ time: reset })}
                  </div>
                {/if}
              {/each}
            </div>
          </section>
        {/each}

        <div class="text-muted-foreground border-t border-border pt-2 text-[10px]">
          {m.session_usage_updated({ time: formatRelativeTime(usage.fetched_at) })}
        </div>
      </div>
    </Popover.Content>
  </Popover.Root>
{/if}
