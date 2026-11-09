<script lang="ts">
  import DiffText from "#lib/components/diff/DiffText.svelte";
  import { m } from "#lib/paraglide/messages.js";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { cn } from "#lib/utils.ts";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import CircleCheckIcon from "@lucide/svelte/icons/circle-check";
  import CircleXIcon from "@lucide/svelte/icons/circle-x";
  import FileSearchIcon from "@lucide/svelte/icons/file-search";
  import HammerIcon from "@lucide/svelte/icons/hammer";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import SearchIcon from "@lucide/svelte/icons/search";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";
  import WrenchIcon from "@lucide/svelte/icons/wrench";

  type ToolItem = Extract<TimelineItem, { kind: "tool" }>;

  interface Props {
    tools: ToolItem[];
  }

  let { tools }: Props = $props();

  let userToggled = $state(false);
  let userIsOpen = $state(false);

  const hasInProgress = $derived(tools.some((t) => t.status === "in_progress"));
  const hasFailed = $derived(tools.some((t) => t.status === "failed"));
  const allCompleted = $derived(tools.every((t) => t.status === "completed"));

  const isOpen = $derived(userToggled ? userIsOpen : hasInProgress || hasFailed);

  let expandedDetails = $state<Record<string, boolean>>({});

  function toggleDetail(toolCallId: string, event: MouseEvent) {
    event.stopPropagation();
    expandedDetails[toolCallId] = !expandedDetails[toolCallId];
  }

  function toggleOpen() {
    userToggled = true;
    userIsOpen = !isOpen;
  }

  function getToolInfo(tool: ToolItem): { name: string; detail: string } {
    const kindMap: Record<string, string> = {
      read: "Read",
      edit: "Edit",
      execute: "Terminal",
      search: "Search",
    };

    // Tool name label (Read, Edit, Terminal, Search, or the tool identifier)
    let name = kindMap[tool.toolKind] ?? "";
    let detail = tool.title;

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
  <!-- Group header -->
  <button
    type="button"
    class="hover:bg-accent/40 flex w-full items-center justify-between px-3 py-2 text-left font-medium select-none"
    onclick={toggleOpen}
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
        {m.chat_tool_calls({ count: tools.length })}
      </span>
    </div>

    <div class="flex items-center gap-2">
      {#if hasInProgress}
        <span class="flex items-center gap-1 text-signal text-[11px]">
          <LoaderCircleIcon class="size-3.5 animate-spin" />
          <span>{m.chat_tool_running()}</span>
        </span>
      {:else if hasFailed}
        <span class="flex items-center gap-1 text-destructive text-[11px]">
          <CircleXIcon class="size-3.5" />
          <span>{m.chat_tool_error()}</span>
        </span>
      {:else if allCompleted}
        <CircleCheckIcon class="size-4 text-ok" />
      {/if}
    </div>
  </button>

  <!-- Tool call list (when expanded) -->
  {#if isOpen}
    <div class="border-border/60 divide-border/60 divide-y border-t bg-muted/20">
      {#each tools as tool (tool.toolCallId)}
        {@const isExpanded = expandedDetails[tool.toolCallId]}
        {@const toolInfo = getToolInfo(tool)}
        {@const hasDetailContent =
          tool.diff !== null ||
          (tool.rawOutput !== null && tool.rawOutput !== "") ||
          tool.locations.length > 0}
        <div class="flex flex-col">
          <!-- Single-line tool summary -->
          <div
            role="button"
            tabindex="0"
            class={cn(
              "flex items-center justify-between px-3 py-1.5 text-[13px] transition-colors",
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
                <FileSearchIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === "edit"}
                <PencilIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === "execute"}
                <SquareTerminalIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else if tool.toolKind === "search"}
                <SearchIcon class="text-muted-foreground size-3.5 shrink-0" />
              {:else}
                <HammerIcon class="text-muted-foreground size-3.5 shrink-0" />
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
            <div class="ml-2 flex shrink-0 items-center gap-2">
              {#if tool.status === "in_progress"}
                <LoaderCircleIcon class="size-3 animate-spin text-signal" />
              {:else if tool.status === "completed"}
                <CircleCheckIcon class="size-3.5 text-ok" />
              {:else if tool.status === "failed"}
                <CircleXIcon class="size-3.5 text-destructive" />
              {/if}

              {#if hasDetailContent}
                <span class="text-muted-foreground text-[11px]">
                  {isExpanded ? "▲" : "▼"}
                </span>
              {/if}
            </div>
          </div>

          <!-- Detail content (diff / log output) -->
          {#if isExpanded && hasDetailContent}
            <div class="bg-background/80 border-border/60 border-t p-2.5 flex flex-col gap-2">
              <div class="flex items-center gap-2 text-[11px]">
                <span class="text-muted-foreground text-[11px]">{m.chat_tool_label()}</span>
                <span class="font-mono font-semibold text-foreground">{toolInfo.name}</span>
                {#if tool.title && tool.title !== toolInfo.name}
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_detail()}</span>
                  <span class="font-mono text-muted-foreground truncate">{tool.title}</span>
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
                    {#each tool.locations as loc (loc)}
                      <li class="truncate">{loc}</li>
                    {/each}
                  </ul>
                </div>
              {/if}

              {#if tool.diff !== null}
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[11px]">Diff:</span>
                  <DiffText diff={tool.diff.unified_diff} maxHeight="12rem" />
                </div>
              {/if}

              {#if tool.rawOutput !== null && tool.rawOutput !== ""}
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[11px]">{m.chat_tool_output()}</span>
                  <pre
                    class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">{tool.rawOutput}</pre>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
