<script lang="ts">
  import ElicitationCard from "#lib/components/ElicitationCard.svelte";
  import PermissionCard from "#lib/components/PermissionCard.svelte";
  import MarkdownText from "#lib/components/MarkdownText.svelte";
  import MessageActions from "./MessageActions.svelte";
  import Self from "./ChatTimeline.svelte";
  import SubagentGroup from "./SubagentGroup.svelte";
  import ThoughtBlock from "./ThoughtBlock.svelte";
  import ToolCallGroup from "./ToolCallGroup.svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { groupTimelineItems } from "#lib/chat-groups.ts";
  import { formatEpochMs } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { ForkScope, SessionStatus } from "#lib/generated/bindings.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import BotIcon from "@lucide/svelte/icons/bot";
  import ClockIcon from "@lucide/svelte/icons/clock";
  import ListChecksIcon from "@lucide/svelte/icons/list-checks";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PaperclipIcon from "@lucide/svelte/icons/paperclip";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

  let {
    sessionId,
    items,
    status = null,
    nested = false,
    onsubagent,
    subagentTitle = null,
    forkScope = null,
    onfork,
  }: {
    sessionId: string;
    items: TimelineItem[];
    /** Session status (derived from events). Used for the running indicator. */
    status?: SessionStatus | null;
    /** Nested rendering inside a subagent block (disables subagent grouping). */
    nested?: boolean;
    onsubagent?: (childSessionId: string) => void;
    /** Title of the enclosing subagent (labels its approvals). */
    subagentTitle?: string | null;
    /** Fork capability reported by the session projection. */
    forkScope?: ForkScope | null;
    /** Called when the user chooses "Fork from here" on a message. */
    onfork?: (item: TimelineItem) => void;
  } = $props();

  /**
   * The agent is running a turn (generating a response or executing tools).
   *
   * The working indicator stays at the end of the timeline not only from send
   * until the first output, but until the turn ends (idle / stopped / error).
   * Waiting for approval (`waiting_permission`) is omitted because it depends on
   * user action.
   */
  const agentWorking = $derived(status === "running");

  /**
   * Whether the timeline tail already shows live activity.
   *
   * A streaming message / reasoning block or a running tool card carries its own
   * progress UI, so the generic "agent is working" row would only duplicate it.
   * It still appears while the agent boots, thinks before the first token, or
   * waits for something that shows nothing yet.
   */
  const tailIsLive = $derived.by(() => {
    const last = items.at(-1);
    if (last === undefined) return false;
    switch (last.kind) {
      case "agent":
      case "thought":
        return last.streaming;
      case "tool":
        return last.status === "pending" || last.status === "in_progress";
      default:
        return false;
    }
  });

  function planStatusClass(planStatus: string): string {
    switch (planStatus) {
      case "completed":
        return "text-muted-foreground line-through";
      case "in_progress":
        return "font-medium";
      default:
        return "text-muted-foreground";
    }
  }

  const groupedItems = $derived(groupTimelineItems(items, { groupSubagents: !nested }));

  const sessionBusy = $derived(
    status === "running" || status === "waiting_permission" || status === "waiting_input",
  );

  /**
   * Latest message that can represent the current end of the conversation.
   * `last` scope only enables Fork there; `message` scope (future) enables all.
   */
  const lastForkableSeq = $derived.by(() => {
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const item = items[index];
      if (item.kind === "user") return item.seq;
      // Subagent messages are nested; only the parent conversation end is a
      // valid "last" fork point for the session.
      if (item.kind === "agent" && item.subagentId === null && !item.streaming) return item.seq;
    }
    return null;
  });

  function forkState(item: TimelineItem): { canFork: boolean | null; reason: string | null } {
    if (nested || onfork === undefined || forkScope === null || forkScope === "none") {
      return { canFork: null, reason: null };
    }
    if (sessionBusy || (item.kind === "agent" && item.streaming)) {
      return { canFork: false, reason: m.chat_action_fork_running() };
    }
    if (forkScope === "message") {
      return { canFork: true, reason: null };
    }
    return item.seq === lastForkableSeq
      ? { canFork: true, reason: null }
      : { canFork: false, reason: m.chat_action_fork_latest() };
  }
</script>

<ul class="flex list-none flex-col gap-4 p-0">
  {#each groupedItems as group (group.key)}
    <li>
      {#if group.kind === "subagent"}
        <SubagentGroup subagent={group.subagent} onopen={onsubagent}>
          <Self
            {sessionId}
            items={group.items}
            {status}
            nested
            {onsubagent}
            subagentTitle={group.subagent.title ?? group.subagent.description ?? null}
          />
        </SubagentGroup>
      {:else if group.kind === "tools"}
        <ToolCallGroup tools={group.tools} />
      {:else}
        {@const item = group.item}
        {#if item.kind === "user"}
          {@const fork = forkState(item)}
          <div class="message-group group relative flex items-end justify-end gap-1.5">
            <MessageActions
              text={item.text}
              canFork={fork.canFork}
              forkReason={fork.reason}
              onfork={() => onfork?.(item)}
            />
            <div
              class="bg-user-message text-foreground max-w-[82%] rounded-xl rounded-br-sm py-2.5 pl-4 pr-8 md:pr-4"
            >
              {#if item.attachments.length > 0}
                <div class="mb-1 flex flex-wrap gap-1.5">
                  {#each item.attachments as attachment (attachment.file_name + (attachment.local_path ?? ""))}
                    {#if attachment.data_b64 && attachment.mime_type.startsWith("image/")}
                      <img
                        src={`data:${attachment.mime_type};base64,${attachment.data_b64}`}
                        alt={attachment.file_name}
                        class="max-h-40 rounded-md object-contain"
                      />
                    {:else}
                      <span
                        class="bg-background/60 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px]"
                      >
                        <PaperclipIcon class="size-2.5" />
                        {attachment.file_name}
                      </span>
                    {/if}
                  {/each}
                </div>
              {/if}
              <p class="text-sm whitespace-pre-wrap">{item.text}</p>
              <p class="text-muted-foreground desk-mono mt-1 text-right text-[11px]">
                {item.clientSource} &middot; {formatEpochMs(item.createdAt)}
              </p>
            </div>
          </div>
        {:else if item.kind === "agent"}
          {@const fork = forkState(item)}
          <div class="message-group group relative flex gap-2.5">
            <MessageActions
              text={item.text}
              canFork={fork.canFork}
              forkReason={fork.reason}
              onfork={() => onfork?.(item)}
            />
            <span
              class="bg-signal-soft text-signal mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md"
            >
              <BotIcon class="size-3" />
            </span>
            <div class="min-w-0 max-w-[92%] flex-1 pr-8 md:pr-0">
              <MarkdownText
                text={item.text}
                streaming={item.streaming}
                class="text-sm leading-relaxed"
              />
            </div>
          </div>
        {:else if item.kind === "thought"}
          <ThoughtBlock {item} />
        {:else if item.kind === "terminal"}
          <div class="bg-card flex flex-col gap-1.5 rounded-lg border border-border p-3">
            <div class="flex items-center gap-2 text-[13px]">
              <SquareTerminalIcon class="text-muted-foreground size-4" />
              <span class="desk-mono">{item.command === "" ? item.terminalId : item.command}</span>
              {#if item.exitCode !== null}
                <Badge variant={item.exitCode === 0 ? "secondary" : "destructive"}>
                  exit {item.exitCode}
                </Badge>
              {/if}
            </div>
            <pre
              class="bg-muted/40 max-h-56 overflow-auto rounded-md border border-border p-2.5 font-mono text-[13px] leading-relaxed whitespace-pre-wrap">{item.output}</pre>
          </div>
        {:else if item.kind === "permission"}
          <PermissionCard
            {sessionId}
            requestId={item.requestId}
            toolName={item.toolName}
            summary={item.summary}
            options={item.options}
            details={item.details}
            resolved={item.resolved}
            createdAt={item.createdAt}
            {subagentTitle}
          />
        {:else if item.kind === "elicitation"}
          <ElicitationCard
            {sessionId}
            elicitationId={item.elicitationId}
            message={item.message}
            requestedSchema={item.requestedSchema}
            resolved={item.resolved}
            createdAt={item.createdAt}
          />
        {:else if item.kind === "plan"}
          <div class="bg-card flex flex-col gap-1.5 rounded-lg border border-border p-3">
            <div class="flex items-center gap-2 text-[13px] font-medium">
              <ListChecksIcon class="size-4" />
              {m.chat_plan()}
            </div>
            <ul class="flex list-none flex-col gap-1 p-0 text-sm">
              {#each item.entries as entry (entry.id)}
                <li class={planStatusClass(entry.status)}>{entry.title}</li>
              {/each}
            </ul>
          </div>
        {:else if item.kind === "subagent_message"}
          <div class="border-border bg-card/60 rounded-lg border px-3 py-2 text-[13px]">
            <p class="text-muted-foreground mb-1 text-[11px]">
              {item.senderSessionId} → {item.recipientSessionId}
            </p>
            <MarkdownText text={item.text} streaming={item.streaming} />
          </div>
        {:else if item.kind === "notice"}
          <div
            class="flex items-start gap-2 rounded-lg border p-3 text-sm {item.tone === 'error'
              ? 'border-danger/30 bg-danger-soft/50 text-danger'
              : item.tone === 'warning'
                ? 'border-warn/30 bg-warn-soft/50 text-warn'
                : 'border-border bg-card text-foreground'}"
          >
            {#if item.tone === "error" || item.tone === "warning"}
              <AlertTriangleIcon class="mt-0.5 size-4 shrink-0" />
            {:else}
              <ClockIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
            {/if}
            <p class="whitespace-pre-wrap">{item.text}</p>
          </div>
        {:else if item.kind === "pending"}
          <div class="text-muted-foreground flex items-center gap-2 text-sm" role="status">
            <LoaderCircleIcon class="text-signal size-4 animate-spin" />
            <span>
              {m.chat_pending_label()}:
              <span class="whitespace-pre-wrap">{item.text}</span>
            </span>
          </div>
        {/if}
      {/if}
    </li>
  {/each}

  {#if agentWorking && !nested && !tailIsLive}
    <li class="flex gap-2.5" role="status" aria-live="polite">
      <span
        class="bg-signal-soft text-signal mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md"
      >
        <BotIcon class="size-3" />
      </span>
      <div
        class="bg-card text-muted-foreground flex items-center gap-2 rounded-lg border border-border px-3 py-1.5 text-[13px]"
      >
        <LoaderCircleIcon class="text-signal size-3.5 animate-spin" />
        <span>{m.chat_agent_working()}</span>
      </div>
    </li>
  {/if}
</ul>

{#if items.length === 0 && !agentWorking && !nested}
  <p class="text-muted-foreground text-[13px]">
    {m.chat_empty_timeline()}
  </p>
{/if}
