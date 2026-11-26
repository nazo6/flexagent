<script lang="ts">
  import DiffText from "#lib/components/diff/DiffText.svelte";
  import { joinPath } from "#lib/dock/paths.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { motion } from "#lib/motion.svelte.ts";
  import { dock } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { parseToolOutput } from "#lib/tool-output.ts";
  import { cn } from "#lib/utils.ts";
  import { SvelteSet } from "svelte/reactivity";
  import { onDestroy } from "svelte";
  import { fade, slide } from "svelte/transition";
  import CircleCheckIcon from "@lucide/svelte/icons/circle-check";
  import CircleXIcon from "@lucide/svelte/icons/circle-x";
  import FileSearchIcon from "@lucide/svelte/icons/file-search";
  import HammerIcon from "@lucide/svelte/icons/hammer";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import SearchIcon from "@lucide/svelte/icons/search";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

  type ToolItem = Extract<TimelineItem, { kind: "tool" }>;

  interface Props {
    tools: ToolItem[];
  }

  let { tools }: Props = $props();

  let expandedDetails = $state<Record<string, boolean>>({});
  const canOpenInDock = $derived(dock.context !== null);

  /**
   * Tool calls that just resolved while the reader was watching.
   *
   * History renders fully populated tool calls, so animating every status
   * icon on mount would make opening a session noisy. Only a transition from
   * `in_progress` to `completed` / `failed` inside a mounted group gets the
   * one-shot pop.
   */
  const statusFlash = new SvelteSet<string>();
  const lastStatus = new Map<string, ToolItem["status"]>();
  const flashTimers = new Set<ReturnType<typeof setTimeout>>();

  $effect(() => {
    for (const tool of tools) {
      const previous = lastStatus.get(tool.toolCallId);
      lastStatus.set(tool.toolCallId, tool.status);
      if (previous === undefined || previous === tool.status) continue;
      if (tool.status !== "completed" && tool.status !== "failed") continue;
      statusFlash.add(tool.toolCallId);
      const timer = setTimeout(() => {
        statusFlash.delete(tool.toolCallId);
        flashTimers.delete(timer);
      }, 300);
      flashTimers.add(timer);
    }
  });

  onDestroy(() => {
    for (const timer of flashTimers) clearTimeout(timer);
    flashTimers.clear();
  });

  function openDiffInDock(tool: ToolItem): void {
    if (tool.diff === null || dock.context === null) return;
    dock.openFile(joinPath(dock.context.localPath, tool.diff.path), { mode: "diff" });
  }

  function toggleDetail(toolCallId: string, event: MouseEvent) {
    event.stopPropagation();
    expandedDetails[toolCallId] = !expandedDetails[toolCallId];
  }

  function getToolInfo(tool: ToolItem): { name: string; detail: string } {
    const kindMap: Record<string, string> = {
      read: "Read",
      edit: "Edit",
      execute: "Terminal",
      search: "Search",
    };

    let name = kindMap[tool.toolKind] ?? "";
    let detail = tool.title;

    if (tool.toolKind === "execute") {
      // The agent sets the shell command as the title; do not split it at the
      // first colon (commands commonly contain one).
      return {
        name: name || "Terminal",
        detail: tool.title || tool.toolCallId,
      };
    }

    if (tool.title.startsWith("Running ")) {
      const funcName = tool.title.replace("Running ", "").trim();
      if (!name) name = funcName;
      detail = tool.locations.length > 0 ? tool.locations.join(", ") : funcName;
    } else if (tool.title.includes(":")) {
      const colonIdx = tool.title.indexOf(":");
      const prefix = tool.title.slice(0, colonIdx).trim();
      const rest = tool.title.slice(colonIdx + 1).trim();
      if (!name) name = prefix;
      detail = rest || (tool.locations.length > 0 ? tool.locations.join(", ") : prefix);
    } else if (tool.locations.length > 0) {
      if (!name) name = tool.title || "Tool";
      detail = tool.locations.join(", ");
    }

    if (!name) {
      name = tool.title || "Tool";
    }

    return {
      name,
      detail: detail || tool.title || tool.toolCallId,
    };
  }
</script>

<div class="bg-card border-border overflow-hidden rounded-lg border text-[13px] transition-colors">
  <!-- Tool call list -->
  <div class="divide-border/60 divide-y bg-muted/20">
    {#each tools as tool (tool.toolCallId)}
      {@const isExpanded = expandedDetails[tool.toolCallId]}
      {@const toolInfo = getToolInfo(tool)}
      {@const isTerminal = tool.toolKind === "execute"}
      {@const result = isTerminal ? parseToolOutput(tool.rawOutput) : null}
      {@const hasRawOutput = tool.rawOutput !== null && tool.rawOutput !== ""}
      {@const terminalFailed =
        result !== null &&
        (tool.status === "failed" ||
          result.error !== null ||
          (result.exitCode !== null && result.exitCode !== 0))}
      {@const hasDetailContent =
        tool.diff !== null ||
        tool.locations.length > 0 ||
        (result !== null
          ? result.text !== "" || result.error !== null || hasRawOutput
          : hasRawOutput)}
      <div class="flex flex-col">
        <!-- Single-line tool summary -->
        <div
          role="button"
          tabindex="0"
          class={cn(
            "flex items-center justify-between gap-2 px-2.5 py-1 text-[12px] transition-colors",
            hasDetailContent ? "cursor-pointer hover:bg-accent/30" : "cursor-default",
          )}
          onclick={(e) => {
            if (hasDetailContent) toggleDetail(tool.toolCallId, e);
          }}
          onkeydown={(e) => {
            if (hasDetailContent && (e.key === "Enter" || e.key === " ")) {
              e.preventDefault();
              expandedDetails[tool.toolCallId] = !expandedDetails[tool.toolCallId];
            }
          }}
        >
          <div class="flex min-w-0 flex-1 items-center gap-2">
            <!-- Icon -->
            {#if tool.toolKind === "read"}
              <FileSearchIcon class="text-muted-foreground size-3 shrink-0" />
            {:else if tool.toolKind === "edit"}
              <PencilIcon class="text-muted-foreground size-3 shrink-0" />
            {:else if tool.toolKind === "execute"}
              <SquareTerminalIcon class="text-muted-foreground size-3 shrink-0" />
            {:else if tool.toolKind === "search"}
              <SearchIcon class="text-muted-foreground size-3 shrink-0" />
            {:else}
              <HammerIcon class="text-muted-foreground size-3 shrink-0" />
            {/if}

            <!-- Tool name badge (Read, Edit, Terminal, Search, etc.) -->
            <span
              class="rounded bg-muted px-1.5 py-0.2 font-mono text-[11px] font-semibold text-foreground/85 shrink-0 border border-border/60"
            >
              {toolInfo.name}
            </span>

            <!-- Command / arguments / target paths -->
            <span class="truncate font-mono text-[11px] text-foreground/90">
              {toolInfo.detail}
            </span>

            {#if tool.locations.length > 0 && !toolInfo.detail.includes(tool.locations[0])}
              <span class="text-muted-foreground truncate font-mono text-[11px]">
                ({tool.locations.join(", ")})
              </span>
            {/if}
          </div>

          <!-- Status & expand toggle -->
          <div class="ml-1.5 flex shrink-0 items-center gap-1.5">
            {#if tool.terminalId}
              <a
                href={`#terminal-${tool.terminalId}`}
                class="border-border/60 bg-muted/50 text-muted-foreground hover:text-foreground inline-flex items-center gap-1 rounded border px-1.5 py-0.2 text-[10px] transition-colors"
                title={m.chat_tool_terminal()}
              >
                <SquareTerminalIcon class="size-3" />
                {m.chat_tool_terminal()}
              </a>
            {/if}
            {#if tool.diff !== null && (tool.diff.additions > 0 || tool.diff.deletions > 0)}
              <span
                class="border-border/60 bg-muted/50 inline-flex items-center gap-1 rounded border px-1.5 py-0.2 font-mono text-[10px] tabular-nums"
              >
                {#if tool.diff.additions > 0}
                  <span class="text-ok">+{tool.diff.additions}</span>
                {/if}
                {#if tool.diff.deletions > 0}
                  <span class="text-danger">-{tool.diff.deletions}</span>
                {/if}
              </span>
            {/if}
            {#if isTerminal && result !== null}
              {#if tool.status === "in_progress"}
                <LoaderCircleIcon class="size-3 animate-spin text-signal" />
              {:else if tool.status === "completed" || tool.status === "failed" || result.exitCode !== null}
                <span
                  class={cn(
                    "rounded border px-1.5 py-0.2 font-mono text-[10px] tabular-nums",
                    statusFlash.has(tool.toolCallId) && "motion-status-pop",
                    terminalFailed
                      ? "border-destructive/30 bg-danger-soft/60 text-destructive"
                      : "border-ok/30 bg-ok-soft/60 text-ok",
                  )}
                >
                  {#if result.exitCode !== null}
                    {m.chat_tool_exit_code({ code: result.exitCode })}
                  {:else}
                    {terminalFailed ? m.chat_tool_failed() : m.chat_tool_done()}
                  {/if}
                </span>
              {:else if tool.status === "pending"}
                <span class="text-muted-foreground text-[10px]">{m.chat_tool_pending()}</span>
              {/if}
            {:else if tool.status === "in_progress"}
              <LoaderCircleIcon class="size-3 animate-spin text-signal" />
            {:else if tool.status === "completed"}
              <CircleCheckIcon
                class={cn(
                  "size-3 text-ok",
                  statusFlash.has(tool.toolCallId) && "motion-status-pop",
                )}
              />
            {:else if tool.status === "failed"}
              <CircleXIcon
                class={cn(
                  "size-3 text-destructive",
                  statusFlash.has(tool.toolCallId) && "motion-status-pop",
                )}
              />
            {/if}

            {#if hasDetailContent}
              <span class="text-muted-foreground text-[10px]">
                {isExpanded ? "▲" : "▼"}
              </span>
            {/if}
          </div>
        </div>

        <!-- Detail content (diff / log output) -->
        {#if isExpanded && hasDetailContent}
          <div transition:slide|local={{ duration: motion.duration(160) }}>
            <div
              transition:fade|local={{ duration: motion.duration(160) }}
              class="bg-background/80 border-border/60 border-t p-2.5 flex flex-col gap-2"
            >
              <div class="flex flex-wrap items-center gap-2 text-[11px]">
                {#if result !== null}
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_command()}</span>
                  <span class="font-mono font-semibold text-foreground">{toolInfo.detail}</span>
                {:else}
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_label()}</span>
                  <span class="font-mono font-semibold text-foreground">{toolInfo.name}</span>
                  {#if tool.title && tool.title !== toolInfo.name}
                    <span class="text-muted-foreground text-[11px]">{m.chat_tool_detail()}</span>
                    <span class="font-mono text-muted-foreground truncate">{tool.title}</span>
                  {/if}
                {/if}
                {#if tool.status}
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_status()}</span>
                  <span class="font-mono text-muted-foreground">{tool.status}</span>
                {/if}
              </div>

              {#if tool.locations.length > 0}
                <div class="flex flex-col gap-0.5 text-[11px]">
                  <span class="text-muted-foreground text-[11px]">
                    {m.chat_tool_target_files()}
                  </span>
                  <ul class="flex flex-col gap-0.5 font-mono text-muted-foreground p-0 list-none">
                    {#each tool.locations as loc, index (`${index}:${loc}`)}
                      <li class="truncate">{loc}</li>
                    {/each}
                  </ul>
                </div>
              {/if}

              {#if tool.diff !== null}
                <div class="flex flex-col gap-1">
                  <div class="flex items-center gap-2">
                    <span class="text-muted-foreground text-[11px]">Diff:</span>
                    {#if canOpenInDock}
                      <button
                        type="button"
                        class="text-signal text-[11px] font-medium hover:underline"
                        onclick={() => openDiffInDock(tool)}
                      >
                        {m.chat_tool_open_in_dock()}
                      </button>
                    {/if}
                  </div>
                  <DiffText diff={tool.diff.unified_diff} maxHeight="12rem" />
                </div>
              {/if}

              {#if result !== null}
                {#if result.error !== null}
                  <div class="flex flex-col gap-1">
                    <span class="text-muted-foreground text-[11px]">{m.chat_tool_error()}</span>
                    <pre
                      class="border-destructive/30 bg-danger-soft/40 text-destructive max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{result.error}</pre>
                  </div>
                {/if}

                {#if result.text !== ""}
                  <div class="flex flex-col gap-1">
                    <span class="text-muted-foreground text-[11px]">{m.chat_tool_output()}</span>
                    <pre
                      class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{result.text}</pre>
                  </div>
                {:else if result.error === null && hasRawOutput}
                  <div class="flex flex-col gap-1">
                    <span class="text-muted-foreground text-[11px]">{m.chat_tool_output()}</span>
                    <pre
                      class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{tool.rawOutput}</pre>
                  </div>
                {/if}

                {#if result.truncated}
                  <span class="text-muted-foreground text-[10px]">
                    {m.chat_tool_output_truncated()}
                  </span>
                {/if}
              {:else if hasRawOutput}
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_output()}</span>
                  <pre
                    class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{tool.rawOutput}</pre>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>
    {/each}
  </div>
</div>
