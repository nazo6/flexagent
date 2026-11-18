import {
  createEffect,
  createMemo,
  createSignal,
  For,
  Match,
  Show,
  Switch,
  untrack,
} from "solid-js";
import AgentIcon from "#lib/components/agent/AgentIcon.tsx";
import * as Popover from "#lib/components/ui/popover/index.ts";
import type { AgentUsageLimits, UsageGroup, UsageWindow } from "#lib/generated/bindings.ts";
import { formatDurationMs, formatRelativeTime } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import IconCheck from "~icons/lucide/check";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconTriangleAlert from "~icons/lucide/triangle-alert";

export interface AgentUsageProps {
  /** Node that owns the agent process. */
  nodeId: string;
  /** Agent ID of the session. */
  agentId: string;
  /** Working directory used for agent-specific lookups. */
  localPath: string;
  /** Agent icon from the catalog (kept even when usage is unavailable). */
  icon?: string | null;
  class?: string;
}

type RefreshState = "idle" | "refreshing" | "updated" | "failed";

/** Flattened window with the least remaining quota (used for the chip). */
interface WorstWindow {
  group: UsageGroup;
  window: UsageWindow;
  remaining: number;
}

/**
 * Agent quota chip.
 *
 * Usage limits are an optional, out-of-session probe: a failure (agent not
 * running, no login, unsupported build) hides the meter instead of surfacing a
 * session error.
 */
export default function AgentUsage(props: AgentUsageProps) {
  const REFRESH_MS = 60_000;

  const [usage, setUsage] = createSignal<AgentUsageLimits | null>(null);
  const [refreshState, setRefreshState] = createSignal<RefreshState>("idle");
  const [hasLoaded, setHasLoaded] = createSignal(false);
  let statusTimer: ReturnType<typeof setTimeout> | null = null;
  let requestId = 0;

  /** The agent identity icon; the source icon is only a fallback. */
  const icon = () => props.icon ?? usage()?.source.icon ?? null;

  function flashStatus(state: "updated" | "failed"): void {
    setRefreshState(state);
    if (statusTimer !== null) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      setRefreshState("idle");
      statusTimer = null;
    }, 2500);
  }

  async function load(
    node = props.nodeId,
    agent = props.agentId,
    path = props.localPath,
    options: { manual?: boolean; force?: boolean } = {},
  ): Promise<void> {
    if (node === "" || agent === "") return;
    if (options.manual && refreshState() === "refreshing") return;
    if (options.manual) setRefreshState("refreshing");
    const current = ++requestId;
    try {
      const response = await sync.connection.client.agentUsage(
        node,
        agent,
        path,
        options.force ?? false,
      );
      if (current === requestId) {
        setUsage(response.usage);
        if (options.manual) flashStatus("updated");
      }
    } catch {
      // Keep the last snapshot; the next refresh may succeed.
      if (current === requestId && options.manual) flashStatus("failed");
    } finally {
      if (current === requestId) setHasLoaded(true);
      // A background poll may supersede this request; do not leave the button
      // spinning forever.
      if (current !== requestId && refreshState() === "refreshing") {
        setRefreshState("idle");
      }
    }
  }

  createEffect(
    () => ({ node: props.nodeId, agent: props.agentId, path: props.localPath }),
    ({ node, agent, path }) => {
      if (node === "" || agent === "") return;
      untrack(() => void load(node, agent, path));
      const timer = setInterval(() => void load(node, agent, path), REFRESH_MS);
      return () => {
        clearInterval(timer);
        if (statusTimer !== null) {
          clearTimeout(statusTimer);
          statusTimer = null;
        }
      };
    },
  );

  const worst = createMemo((): WorstWindow | null => {
    const data = usage();
    if (data === null) return null;
    let result: WorstWindow | null = null;
    for (const group of data.groups) {
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

  /** Full summary used by the icon's tooltip and accessible name. */
  const summaryLabel = createMemo(() => {
    const data = usage();
    const current = worst();
    if (data === null || current === null) return m.session_usage_title();
    const group = data.groups.length > 1 ? `${current.group.label} · ` : "";
    if (current.window.limited) {
      return `${data.source.label} · ${group}${m.session_usage_limited()}`;
    }
    return `${data.source.label} · ${group}${windowLabel(current.window)} ${m.session_usage_left({
      percent: remainingPercent(current.window),
    })}`;
  });

  return (
    <Popover.Root>
      <Popover.Trigger
        class={cn(
          "text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/40 border-border flex size-7 shrink-0 items-center justify-center rounded-md border outline-none transition-colors focus-visible:ring-2",
          props.class,
        )}
        title={summaryLabel()}
        aria-label={summaryLabel()}
      >
        <AgentIcon icon={icon()} class="size-4" />
      </Popover.Trigger>

      <Popover.Content align="start" class="w-[min(23rem,calc(100vw-2rem))] p-0">
        <div class="border-border border-b px-3 py-2">
          <div class="flex min-w-0 items-center gap-2">
            <AgentIcon icon={icon()} class="size-4" />
            <div class="min-w-0">
              <div class="flex min-w-0 items-center gap-1.5">
                <h2 class="truncate text-[13px] font-semibold">
                  {usage()?.source.label ?? m.session_usage_title()}
                </h2>
                <Show when={usage()?.source.plan}>
                  {(plan) => (
                    <span class="bg-muted text-muted-foreground rounded px-1.5 py-0.5 text-[10px]">
                      {plan()}
                    </span>
                  )}
                </Show>
              </div>
              <div class="text-muted-foreground truncate text-[11px]">
                {usage()?.source.id ?? props.agentId}
                {usage()?.source.account ? ` · ${usage()?.source.account}` : ""}
              </div>
            </div>
            <button
              type="button"
              class="text-muted-foreground hover:text-foreground hover:bg-accent ml-auto flex size-6 shrink-0 items-center justify-center rounded-md transition-colors"
              title={m.session_usage_refresh()}
              aria-label={m.session_usage_refresh()}
              onClick={() =>
                void load(props.nodeId, props.agentId, props.localPath, {
                  manual: true,
                  force: true,
                })
              }
              disabled={refreshState() === "refreshing"}
            >
              <IconRefreshCw
                class={cn("size-3.5", refreshState() === "refreshing" && "animate-spin")}
              />
            </button>
          </div>
        </div>

        <Show
          when={usage()}
          fallback={
            <div class="text-muted-foreground p-3 text-[12px] leading-snug" aria-live="polite">
              <Switch>
                <Match when={!hasLoaded() || refreshState() === "refreshing"}>
                  <span class="flex items-center gap-1.5">
                    <IconRefreshCw class="size-3 animate-spin" />
                    {m.session_usage_checking()}
                  </span>
                </Match>
                <Match when={refreshState() === "failed"}>
                  <span class="text-destructive flex items-center gap-1.5">
                    <IconTriangleAlert class="size-3" />
                    {m.session_usage_update_failed()}
                  </span>
                </Match>
                <Match when={true}>{m.session_usage_unavailable()}</Match>
              </Switch>
            </div>
          }
        >
          {(data) => (
            <div class="space-y-3 p-3">
              <For each={data().groups}>
                {(group) => (
                  <section>
                    <div class="flex items-baseline justify-between gap-2">
                      <h3 class="text-[12px] font-semibold">{group.label}</h3>
                    </div>
                    <Show when={group.description}>
                      {(description) => (
                        <p class="text-muted-foreground mt-0.5 text-[10.5px] leading-snug">
                          {description()}
                        </p>
                      )}
                    </Show>
                    <div class="mt-1.5 space-y-1.5">
                      <For each={group.windows}>
                        {(window) => (
                          <>
                            <div class="grid grid-cols-[3.4rem_minmax(3rem,1fr)_4.5rem] items-center gap-2">
                              <span class="text-[11px] font-medium">{windowLabel(window)}</span>
                              <span class="bg-muted h-1.5 overflow-hidden rounded-full">
                                <span
                                  class={cn("block h-full rounded-full", barClass(window))}
                                  style={{ width: `${remainingPercent(window)}%` }}
                                />
                              </span>
                              <span class="text-right font-mono text-[10.5px] tabular-nums">
                                <Show
                                  when={window.limited}
                                  fallback={
                                    <>
                                      {m.session_usage_left({
                                        percent: remainingPercent(window),
                                      })}
                                    </>
                                  }
                                >
                                  <span class="text-destructive">{m.session_usage_limited()}</span>
                                </Show>
                              </span>
                            </div>
                            <Show when={resetsIn(window)}>
                              {(reset) => (
                                <div class="text-muted-foreground -mt-1 text-right text-[10px]">
                                  {m.session_usage_resets_in({ time: reset() })}
                                </div>
                              )}
                            </Show>
                          </>
                        )}
                      </For>
                    </div>
                  </section>
                )}
              </For>

              <div
                class="text-muted-foreground border-border flex items-center gap-1 border-t pt-2 text-[10px]"
                aria-live="polite"
              >
                <Show when={refreshState() === "refreshing"}>
                  <IconRefreshCw class="size-3 animate-spin" />
                  <span>{m.session_usage_checking()}</span>
                </Show>
                <Show when={refreshState() === "updated"}>
                  <IconCheck class="text-ok size-3" />
                  <span>{m.session_usage_updated_just_now()}</span>
                </Show>
                <Show when={refreshState() === "failed"}>
                  <IconTriangleAlert class="text-destructive size-3" />
                  <span class="text-destructive">{m.session_usage_update_failed()}</span>
                </Show>
                <Show when={refreshState() === "idle"}>
                  <span>
                    {m.session_usage_updated({ time: formatRelativeTime(data().fetched_at) })}
                  </span>
                </Show>
              </div>
            </div>
          )}
        </Show>
      </Popover.Content>
    </Popover.Root>
  );
}
