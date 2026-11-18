import { createStore } from "solid-js";
import { Show } from "solid-js";
import { For } from "solid-js";
import DiffText from "#lib/components/diff/DiffText.tsx";
import { joinPath } from "#lib/dock/paths.ts";
import { m } from "#lib/paraglide/messages.js";
import { dock } from "#lib/stores/dock.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import { summarizeToolOutput } from "#lib/tool-output.ts";
import { cn } from "#lib/utils.ts";
import IconCircleCheck from "~icons/lucide/circle-check";
import IconCircleX from "~icons/lucide/circle-x";
import IconCornerDownRight from "~icons/lucide/corner-down-right";
import IconFileSearch from "~icons/lucide/file-search";
import IconHammer from "~icons/lucide/hammer";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPencil from "~icons/lucide/pencil";
import IconSearch from "~icons/lucide/search";
import IconSquareTerminal from "~icons/lucide/square-terminal";

type ToolItem = Extract<TimelineItem, { kind: "tool" }>;

export interface ToolCallGroupProps {
  tools: ToolItem[];
}

/** One card with the consecutive tool calls of a turn. */
export default function ToolCallGroup(props: ToolCallGroupProps) {
  const [expandedDetails, setExpandedDetails] = createStore<Record<string, boolean>>({});
  const canOpenInDock = () => dock.context !== null;

  function openDiffInDock(tool: ToolItem): void {
    const context = dock.context;
    if (tool.diff === null || context === null) return;
    dock.openFile(joinPath(context.localPath, tool.diff.path), { mode: "diff" });
  }

  function toggleDetail(toolCallId: string, event: MouseEvent): void {
    event.stopPropagation();
    setExpandedDetails((current) => {
      current[toolCallId] = !current[toolCallId];
    });
  }

  function setDetail(toolCallId: string): void {
    setExpandedDetails((current) => {
      current[toolCallId] = !current[toolCallId];
    });
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

  return (
    <div class="bg-card border-border overflow-hidden rounded-lg border text-[13px] transition-colors">
      {/* Tool call list */}
      <div class="divide-border/60 divide-y bg-muted/20">
        <For each={props.tools}>
          {(tool) => {
            const toolInfo = getToolInfo(tool);
            const isTerminal = tool.toolKind === "execute";
            const result = isTerminal ? summarizeToolOutput(tool.rawOutput, tool.status) : null;
            const hasRawOutput = tool.rawOutput !== null && tool.rawOutput !== "";
            const terminalFailed =
              result !== null &&
              (tool.status === "failed" ||
                result.error !== null ||
                (result.exitCode !== null && result.exitCode !== 0));
            const hasDetailContent =
              tool.diff !== null ||
              tool.locations.length > 0 ||
              (result !== null
                ? result.text !== "" || result.error !== null || hasRawOutput
                : hasRawOutput);

            return (
              <div class="flex flex-col">
                {/* Single-line tool summary */}
                <div
                  role="button"
                  tabindex="0"
                  class={cn(
                    "flex items-start justify-between gap-2 px-2.5 py-1 text-[12px] transition-colors",
                    hasDetailContent ? "hover:bg-accent/30 cursor-pointer" : "cursor-default",
                  )}
                  onClick={(event) => {
                    if (hasDetailContent) toggleDetail(tool.toolCallId, event);
                  }}
                  onKeyDown={(event) => {
                    if (hasDetailContent && (event.key === "Enter" || event.key === " ")) {
                      event.preventDefault();
                      setDetail(tool.toolCallId);
                    }
                  }}
                >
                  <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                    <div class="flex min-w-0 items-center gap-2">
                      {/* Icon */}
                      <Show when={tool.toolKind === "read"}>
                        <IconFileSearch class="text-muted-foreground size-3 shrink-0" />
                      </Show>
                      <Show when={tool.toolKind === "edit"}>
                        <IconPencil class="text-muted-foreground size-3 shrink-0" />
                      </Show>
                      <Show when={tool.toolKind === "execute"}>
                        <IconSquareTerminal class="text-muted-foreground size-3 shrink-0" />
                      </Show>
                      <Show when={tool.toolKind === "search"}>
                        <IconSearch class="text-muted-foreground size-3 shrink-0" />
                      </Show>
                      <Show
                        when={
                          tool.toolKind !== "read" &&
                          tool.toolKind !== "edit" &&
                          tool.toolKind !== "execute" &&
                          tool.toolKind !== "search"
                        }
                      >
                        <IconHammer class="text-muted-foreground size-3 shrink-0" />
                      </Show>

                      {/* Tool name badge (Read, Edit, Terminal, Search, etc.) */}
                      <span class="text-foreground/85 border-border/60 shrink-0 rounded border bg-muted px-1.5 py-0.2 font-mono text-[11px] font-semibold">
                        {toolInfo.name}
                      </span>

                      {/* Command / arguments / target paths */}
                      <span class="text-foreground/90 truncate font-mono text-[11px]">
                        {toolInfo.detail}
                      </span>

                      <Show
                        when={
                          tool.locations.length > 0 && !toolInfo.detail.includes(tool.locations[0])
                        }
                      >
                        <span class="text-muted-foreground truncate font-mono text-[11px]">
                          ({tool.locations.join(", ")})
                        </span>
                      </Show>
                    </div>

                    {/* Terminal result preview (visible without expanding the detail) */}
                    <Show when={result !== null && result.preview !== ""}>
                      <div class="flex min-w-0 items-center gap-1.5 pl-5">
                        <IconCornerDownRight class="text-muted-foreground size-3 shrink-0" />
                        <span
                          class={cn(
                            "truncate font-mono text-[11px]",
                            terminalFailed ? "text-destructive/85" : "text-muted-foreground",
                          )}
                        >
                          {result?.preview}
                        </span>
                        <Show when={(result?.hiddenLineCount ?? 0) > 0}>
                          <span class="text-muted-foreground/70 shrink-0 text-[10px]">
                            +{result?.hiddenLineCount}
                          </span>
                        </Show>
                      </div>
                    </Show>
                  </div>

                  {/* Status & expand toggle */}
                  <div class="mt-0.5 ml-1.5 flex shrink-0 items-center gap-1.5">
                    <Show
                      when={
                        tool.diff !== null &&
                        ((tool.diff?.additions ?? 0) > 0 || (tool.diff?.deletions ?? 0) > 0)
                      }
                    >
                      <span class="border-border/60 bg-muted/50 inline-flex items-center gap-1 rounded border px-1.5 py-0.2 font-mono text-[10px] tabular-nums">
                        <Show when={(tool.diff?.additions ?? 0) > 0}>
                          <span class="text-ok">+{tool.diff?.additions}</span>
                        </Show>
                        <Show when={(tool.diff?.deletions ?? 0) > 0}>
                          <span class="text-danger">-{tool.diff?.deletions}</span>
                        </Show>
                      </span>
                    </Show>
                    <Show
                      when={isTerminal && result !== null}
                      fallback={
                        <>
                          <Show when={tool.status === "in_progress"}>
                            <IconLoaderCircle class="text-signal size-3 animate-spin" />
                          </Show>
                          <Show when={tool.status === "completed"}>
                            <IconCircleCheck class="text-ok size-3" />
                          </Show>
                          <Show when={tool.status === "failed"}>
                            <IconCircleX class="text-destructive size-3" />
                          </Show>
                        </>
                      }
                    >
                      <Show when={tool.status === "in_progress"}>
                        <IconLoaderCircle class="text-signal size-3 animate-spin" />
                      </Show>
                      <Show
                        when={
                          tool.status === "completed" ||
                          tool.status === "failed" ||
                          result?.exitCode !== null
                        }
                      >
                        <span
                          class={cn(
                            "rounded border px-1.5 py-0.2 font-mono text-[10px] tabular-nums",
                            terminalFailed
                              ? "border-destructive/30 bg-danger-soft/60 text-destructive"
                              : "border-ok/30 bg-ok-soft/60 text-ok",
                          )}
                        >
                          <Show
                            when={(result?.exitCode ?? null) === null}
                            fallback={m.chat_tool_exit_code({ code: result?.exitCode ?? 0 })}
                          >
                            {terminalFailed ? m.chat_tool_failed() : m.chat_tool_done()}
                          </Show>
                        </span>
                      </Show>
                      <Show when={tool.status === "pending"}>
                        <span class="text-muted-foreground text-[10px]">
                          {m.chat_tool_pending()}
                        </span>
                      </Show>
                    </Show>

                    <Show when={hasDetailContent}>
                      <span class="text-muted-foreground text-[10px]">
                        {expandedDetails[tool.toolCallId] ? "▲" : "▼"}
                      </span>
                    </Show>
                  </div>
                </div>

                {/* Detail content (diff / log output) */}
                <Show when={expandedDetails[tool.toolCallId] === true && hasDetailContent}>
                  <div class="bg-background/80 border-border/60 flex flex-col gap-2 border-t p-2.5">
                    <div class="flex flex-wrap items-center gap-2 text-[11px]">
                      <Show
                        when={result !== null}
                        fallback={
                          <>
                            <span class="text-muted-foreground text-[11px]">
                              {m.chat_tool_label()}
                            </span>
                            <span class="text-foreground font-mono font-semibold">
                              {toolInfo.name}
                            </span>
                            <Show when={tool.title && tool.title !== toolInfo.name}>
                              <span class="text-muted-foreground text-[11px]">
                                {m.chat_tool_detail()}
                              </span>
                              <span class="text-muted-foreground truncate font-mono">
                                {tool.title}
                              </span>
                            </Show>
                          </>
                        }
                      >
                        <span class="text-muted-foreground text-[11px]">
                          {m.chat_tool_command()}
                        </span>
                        <span class="text-foreground font-mono font-semibold">
                          {toolInfo.detail}
                        </span>
                      </Show>
                      <Show when={tool.status}>
                        <span class="text-muted-foreground text-[11px]">
                          {m.chat_tool_status()}
                        </span>
                        <span class="text-muted-foreground font-mono">{tool.status}</span>
                      </Show>
                    </div>

                    <Show when={tool.locations.length > 0}>
                      <div class="flex flex-col gap-0.5 text-[11px]">
                        <span class="text-muted-foreground text-[11px]">
                          {m.chat_tool_target_files()}
                        </span>
                        <ul class="text-muted-foreground flex list-none flex-col gap-0.5 p-0 font-mono">
                          <For each={tool.locations}>
                            {(loc) => <li class="truncate">{loc}</li>}
                          </For>
                        </ul>
                      </div>
                    </Show>

                    <Show when={tool.diff}>
                      {(diff) => (
                        <div class="flex flex-col gap-1">
                          <div class="flex items-center gap-2">
                            <span class="text-muted-foreground text-[11px]">Diff:</span>
                            <Show when={canOpenInDock()}>
                              <button
                                type="button"
                                class="text-signal text-[11px] font-medium hover:underline"
                                onClick={() => openDiffInDock(tool)}
                              >
                                {m.chat_tool_open_in_dock()}
                              </button>
                            </Show>
                          </div>
                          <DiffText diff={diff().unified_diff} maxHeight="12rem" />
                        </div>
                      )}
                    </Show>

                    <Show
                      when={result !== null}
                      fallback={
                        <Show when={hasRawOutput}>
                          <div class="flex flex-col gap-1">
                            <span class="text-muted-foreground text-[11px]">
                              {m.chat_tool_output()}
                            </span>
                            <pre class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">
                              {tool.rawOutput}
                            </pre>
                          </div>
                        </Show>
                      }
                    >
                      <Show when={result?.error}>
                        {(error) => (
                          <div class="flex flex-col gap-1">
                            <span class="text-muted-foreground text-[11px]">
                              {m.chat_tool_error()}
                            </span>
                            <pre class="border-destructive/30 bg-danger-soft/40 text-destructive max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">
                              {error()}
                            </pre>
                          </div>
                        )}
                      </Show>

                      <Show
                        when={result?.text !== ""}
                        fallback={
                          <Show when={(result?.error ?? null) === null && hasRawOutput}>
                            <div class="flex flex-col gap-1">
                              <span class="text-muted-foreground text-[11px]">
                                {m.chat_tool_output()}
                              </span>
                              <pre class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">
                                {tool.rawOutput}
                              </pre>
                            </div>
                          </Show>
                        }
                      >
                        <div class="flex flex-col gap-1">
                          <span class="text-muted-foreground text-[11px]">
                            {m.chat_tool_output()}
                          </span>
                          <pre class="bg-muted/40 max-h-40 overflow-auto rounded-md border p-2 font-mono text-[11px] whitespace-pre-wrap">
                            {result?.text}
                          </pre>
                        </div>
                      </Show>

                      <Show when={result?.truncated}>
                        <span class="text-muted-foreground text-[10px]">
                          {m.chat_tool_output_truncated()}
                        </span>
                      </Show>
                    </Show>
                  </div>
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
