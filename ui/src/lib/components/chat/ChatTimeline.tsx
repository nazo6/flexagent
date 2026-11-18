import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import ElicitationCard from "#lib/components/ElicitationCard.tsx";
import MarkdownText from "#lib/components/MarkdownText.tsx";
import PermissionCard from "#lib/components/PermissionCard.tsx";
import ActivityBlock from "./ActivityBlock.tsx";
import ForkOrigin from "./ForkOrigin.tsx";
import MessageActions from "./MessageActions.tsx";
import SubagentGroup from "./SubagentGroup.tsx";
import ThoughtBlock from "./ThoughtBlock.tsx";
import ToolCallGroup from "./ToolCallGroup.tsx";
import { Badge } from "#lib/components/ui/badge/index.ts";
import {
  groupActivitySections,
  groupTimelineItems,
  type ActivitySection,
  type TimelineEntry,
  type TimelineGroup,
} from "#lib/chat-groups.ts";
import { formatClockTime, formatEpochMs } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import type { ForkScope, SessionStatus } from "#lib/generated/bindings.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import { cn } from "#lib/utils.ts";
import IconAlertTriangle from "~icons/lucide/alert-triangle";
import IconBot from "~icons/lucide/bot";
import IconCircleStop from "~icons/lucide/circle-stop";
import IconClock from "~icons/lucide/clock";
import IconListChecks from "~icons/lucide/list-checks";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPaperclip from "~icons/lucide/paperclip";
import IconSquareTerminal from "~icons/lucide/square-terminal";

export interface ChatTimelineProps {
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
  /** Parent session ID when this session is a fork. */
  forkFrom?: string | null;
  /** Title of the parent session (falls back to a short ID). */
  forkFromTitle?: string | null;
  /** `node_seq` of the last event copied from the parent. */
  forkAfterSeq?: number | null;
  /** Called when the user chooses "Fork from here" on a message. */
  onfork?: (item: TimelineItem) => void;
  /** Whether the agent supports reverting the conversation. */
  canRevert?: boolean;
  /** Called when the user chooses "Revert to here" on a message. */
  onrevert?: (item: TimelineItem) => void;
  /**
   * Called when the user stops a command that is still running. Must resolve
   * when the request finished so the card re-enables the button on failure.
   */
  onstopterminal?: (terminalId: string) => Promise<void>;
}

/**
 * Values the rows need, resolved once per change.
 *
 * A memo hands the rows a plain value object: the `For` bodies are keyed by the
 * reducer's stable item key, so a streamed delta updates the row that owns it
 * instead of remounting the list, and the rows read `props.ctx.*` like any
 * other prop. The memo only recomputes when a value below actually changes, so
 * a delta does not rebuild it.
 */
interface TimelineContext {
  sessionId: string;
  status: SessionStatus | null;
  subagentTitle: string | null;
  onsubagent: ((childSessionId: string) => void) | undefined;
  onfork: ((item: TimelineItem) => void) | undefined;
  onrevert: ((item: TimelineItem) => void) | undefined;
  onstopterminal: ((terminalId: string) => Promise<void>) | undefined;
  stopTerminal: (terminalId: string) => void;
  isStoppingTerminal: (terminalId: string) => boolean;
  sessionStopped: boolean;
  lastForkableSeq: number | null;
  revert: { canRevert: boolean | null; reason: string | null };
  forkState: (item: TimelineItem) => { canFork: boolean | null; reason: string | null };
  liveEntryKey: string | null;
}

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

function groupMaxSeq(group: TimelineGroup): number {
  switch (group.kind) {
    case "subagent":
      return Math.max(group.subagent.seq, ...group.items.map((item) => item.seq));
    case "tools":
      return Math.max(...group.tools.map((tool) => tool.seq));
    default:
      return group.item.seq;
  }
}

/** One row of an activity run. */
function ActivityRow(props: { section: ActivitySection; ctx: TimelineContext }) {
  return (
    <ActivityBlock section={props.section} live={props.ctx.liveEntryKey === props.section.key}>
      <ul class="timeline-list list-none p-0">
        <For each={props.section.groups} keyed={(group) => group.key}>
          {(group) => (
            <li>
              <GroupView group={group()} ctx={props.ctx} />
            </li>
          )}
        </For>
      </ul>
    </ActivityBlock>
  );
}

/** One group: a subagent block, a run of tool calls, or a single timeline item. */
function GroupView(props: { group: TimelineGroup; ctx: TimelineContext }) {
  return (
    <Switch>
      <Match when={props.group.kind === "subagent" ? props.group : null}>
        {(group) => (
          <SubagentGroup subagent={group().subagent} onopen={props.ctx.onsubagent}>
            <ChatTimeline
              sessionId={props.ctx.sessionId}
              items={group().items}
              status={props.ctx.status}
              nested
              onsubagent={props.ctx.onsubagent}
              onstopterminal={props.ctx.onstopterminal}
              subagentTitle={group().subagent.title ?? group().subagent.description ?? null}
            />
          </SubagentGroup>
        )}
      </Match>
      <Match when={props.group.kind === "tools" ? props.group : null}>
        {(group) => <ToolCallGroup tools={group().tools} />}
      </Match>
      <Match when={props.group.kind === "single" ? props.group : null}>
        {(group) => <SingleRow item={group().item} ctx={props.ctx} />}
      </Match>
    </Switch>
  );
}

/** One conversation item (message, reasoning, approval, plan, notice, ...). */
function SingleRow(props: { item: TimelineItem; ctx: TimelineContext }): JSX.Element {
  const ctx = props.ctx;
  return (
    <Switch>
      <Match when={props.item.kind === "user" ? props.item : null}>
        {(item) => (
          <div class="message-group">
            <div class="flex justify-end">
              <div class="bg-user-message text-foreground max-w-[82%] rounded-xl rounded-br-sm px-4 py-2.5">
                <Show when={item().attachments.length > 0}>
                  <div class="mb-1 flex flex-wrap gap-1.5">
                    <For each={item().attachments}>
                      {(attachment) => (
                        <Show
                          when={
                            attachment.data_b64 !== null &&
                            attachment.mime_type.startsWith("image/")
                          }
                          fallback={
                            <span class="bg-background/60 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px]">
                              <IconPaperclip class="size-2.5" />
                              {attachment.file_name}
                            </span>
                          }
                        >
                          <img
                            src={`data:${attachment.mime_type};base64,${attachment.data_b64 ?? ""}`}
                            alt={attachment.file_name}
                            class="max-h-40 rounded-md object-contain"
                          />
                        </Show>
                      )}
                    </For>
                  </div>
                </Show>
                <p class="text-sm whitespace-pre-wrap">{item().text}</p>
              </div>
            </div>
            <MessageActions
              text={item().text}
              canFork={ctx.forkState(item()).canFork}
              forkReason={ctx.forkState(item()).reason}
              canRevert={ctx.revert.canRevert}
              revertReason={ctx.revert.reason}
              prominent={item().seq === ctx.lastForkableSeq}
              meta={`${item().clientSource} · ${formatClockTime(item().createdAt)}`}
              metaTitle={formatEpochMs(item().createdAt)}
              onfork={() => ctx.onfork?.(item())}
              onrevert={() => ctx.onrevert?.(item())}
            />
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "agent" ? props.item : null}>
        {(item) => (
          <div class="message-group">
            <div class="flex gap-2.5">
              <span class="bg-signal-soft text-signal mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md">
                <IconBot class="size-3" />
              </span>
              <div class="min-w-0 max-w-[92%] flex-1">
                <MarkdownText
                  text={item().text}
                  streaming={item().streaming}
                  class="text-sm leading-relaxed"
                />
              </div>
            </div>
            <MessageActions
              text={item().text}
              canFork={ctx.forkState(item()).canFork}
              forkReason={ctx.forkState(item()).reason}
              canRevert={null}
              revertReason={null}
              prominent={item().seq === ctx.lastForkableSeq}
              onfork={() => ctx.onfork?.(item())}
            />
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "thought" ? props.item : null}>
        {(item) => <ThoughtBlock item={item()} />}
      </Match>

      <Match when={props.item.kind === "terminal" ? props.item : null}>
        {(item) => (
          <div class="bg-card border-border flex flex-col gap-1.5 rounded-lg border p-3">
            <div class="flex items-center gap-2 text-[13px]">
              <IconSquareTerminal class="text-muted-foreground size-4" />
              <span class="desk-mono">
                {item().command === "" ? item().terminalId : item().command}
              </span>
              <Show
                when={item().running && !ctx.sessionStopped && ctx.onstopterminal !== undefined}
                fallback={
                  <Show when={item().exitCode !== null}>
                    <Badge variant={item().exitCode === 0 ? "secondary" : "destructive"}>
                      exit {item().exitCode}
                    </Badge>
                  </Show>
                }
              >
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-destructive/10 hover:text-destructive ml-auto flex size-5 shrink-0 items-center justify-center rounded transition-colors disabled:opacity-50"
                  title={
                    ctx.isStoppingTerminal(item().terminalId)
                      ? m.chat_terminal_stopping()
                      : m.chat_terminal_stop()
                  }
                  aria-label={
                    ctx.isStoppingTerminal(item().terminalId)
                      ? m.chat_terminal_stopping()
                      : m.chat_terminal_stop()
                  }
                  disabled={ctx.isStoppingTerminal(item().terminalId)}
                  onClick={() => ctx.stopTerminal(item().terminalId)}
                >
                  <Show
                    when={ctx.isStoppingTerminal(item().terminalId)}
                    fallback={<IconCircleStop class="size-3.5" />}
                  >
                    <IconLoaderCircle class="size-3.5 animate-spin" />
                  </Show>
                </button>
              </Show>
            </div>
            <pre class="bg-muted/40 border-border max-h-56 overflow-auto rounded-md border p-2.5 font-mono text-[13px] leading-relaxed whitespace-pre-wrap">
              {item().output}
            </pre>
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "permission" ? props.item : null}>
        {(item) => (
          <PermissionCard
            sessionId={ctx.sessionId}
            requestId={item().requestId}
            toolName={item().toolName}
            summary={item().summary}
            options={item().options}
            details={item().details}
            resolved={item().resolved}
            createdAt={item().createdAt}
            subagentTitle={ctx.subagentTitle}
          />
        )}
      </Match>

      <Match when={props.item.kind === "elicitation" ? props.item : null}>
        {(item) => (
          <ElicitationCard
            sessionId={ctx.sessionId}
            elicitationId={item().elicitationId}
            message={item().message}
            requestedSchema={item().requestedSchema}
            resolved={item().resolved}
            createdAt={item().createdAt}
          />
        )}
      </Match>

      <Match when={props.item.kind === "plan" ? props.item : null}>
        {(item) => (
          <div class="bg-card border-border flex flex-col gap-1.5 rounded-lg border p-3">
            <div class="flex items-center gap-2 text-[13px] font-medium">
              <IconListChecks class="size-4" />
              {m.chat_plan()}
            </div>
            <ul class="flex list-none flex-col gap-1 p-0 text-sm">
              <For each={item().entries}>
                {(entry) => <li class={planStatusClass(entry.status)}>{entry.title}</li>}
              </For>
            </ul>
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "subagent_message" ? props.item : null}>
        {(item) => (
          <div class="border-border bg-card/60 rounded-lg border px-3 py-2 text-[13px]">
            <p class="text-muted-foreground mb-1 text-[11px]">
              {item().senderSessionId} → {item().recipientSessionId}
            </p>
            <MarkdownText text={item().text} streaming={item().streaming} />
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "notice" ? props.item : null}>
        {(item) => (
          <div
            class={cn(
              "flex items-start gap-2 rounded-lg border p-3 text-sm",
              item().tone === "error"
                ? "border-danger/30 bg-danger-soft/50 text-danger"
                : item().tone === "warning"
                  ? "border-warn/30 bg-warn-soft/50 text-warn"
                  : "border-border bg-card text-foreground",
            )}
          >
            <Show
              when={item().tone === "error" || item().tone === "warning"}
              fallback={<IconClock class="text-muted-foreground mt-0.5 size-4 shrink-0" />}
            >
              <IconAlertTriangle class="mt-0.5 size-4 shrink-0" />
            </Show>
            <p class="whitespace-pre-wrap">{item().text}</p>
          </div>
        )}
      </Match>

      <Match when={props.item.kind === "pending" ? props.item : null}>
        {(item) => (
          <div class="text-muted-foreground flex items-center gap-2 text-sm" role="status">
            <IconLoaderCircle class="text-signal size-4 animate-spin" />
            <span>
              {m.chat_pending_label()}:<span class="whitespace-pre-wrap">{item().text}</span>
            </span>
          </div>
        )}
      </Match>
    </Switch>
  );
}

/** A rendered row: a folded activity run or a single group. */
function TimelineRow(props: { entry: TimelineEntry; ctx: TimelineContext }) {
  return (
    <Switch>
      <Match when={props.entry.kind === "activity" ? props.entry : null}>
        {(section) => <ActivityRow section={section()} ctx={props.ctx} />}
      </Match>
      <Match when={props.entry.kind !== "activity" ? props.entry : null}>
        {(group) => <GroupView group={group()} ctx={props.ctx} />}
      </Match>
    </Switch>
  );
}

/**
 * Conversation timeline.
 *
 * Rows are keyed by the reducer's stable item key, so a streamed delta updates
 * the row that owns it instead of remounting the list. Intermediate work
 * (tools, reasoning, subagents) is folded once per turn by `chat-groups`.
 */
export default function ChatTimeline(props: ChatTimelineProps) {
  const nested = () => props.nested ?? false;
  const status = () => props.status ?? null;
  const forkScope = () => props.forkScope ?? null;
  const forkFrom = () => props.forkFrom ?? null;
  const forkAfterSeq = () => props.forkAfterSeq ?? null;
  const canRevert = () => props.canRevert ?? false;

  /** Terminals whose stop request is in flight (per card). */
  const [stoppingTerminals, setStoppingTerminals] = createSignal<ReadonlySet<string>>(new Set());

  function stopTerminal(terminalId: string): void {
    if (stoppingTerminals().has(terminalId) || props.onstopterminal === undefined) return;
    setStoppingTerminals((current) => new Set(current).add(terminalId));
    void props.onstopterminal(terminalId).finally(() => {
      setStoppingTerminals((current) => {
        if (!current.has(terminalId)) return current;
        const next = new Set(current);
        next.delete(terminalId);
        return next;
      });
    });
  }

  const isStoppingTerminal = (terminalId: string): boolean => stoppingTerminals().has(terminalId);

  /**
   * The agent is running a turn (generating a response or executing tools).
   *
   * The working indicator stays at the end of the timeline not only from send
   * until the first output, but until the turn ends (idle / stopped / error).
   * Waiting for approval (`waiting_permission`) is omitted because it depends on
   * user action.
   */
  const agentWorking = () => status() === "running";

  /**
   * Whether the timeline tail already shows live activity.
   *
   * A streaming message / reasoning block or a running tool card carries its own
   * progress UI, so the generic "agent is working" row would only duplicate it.
   * It still appears while the agent boots, thinks before the first token, or
   * waits for something that shows nothing yet.
   */
  const tailIsLive = createMemo(() => {
    const last = props.items.at(-1);
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

  const groupedItems = createMemo(() =>
    groupTimelineItems(props.items, { groupSubagents: !nested() }),
  );

  /** Group after which the fork origin marker is rendered. */
  const forkAfterGroupKey = createMemo(() => {
    const after = forkAfterSeq();
    if (forkFrom() === null || after === null) return null;
    let key: string | null = null;
    for (const group of groupedItems()) {
      if (groupMaxSeq(group) <= after) key = group.key;
      else break;
    }
    return key;
  });

  /** Rendered rows: activity runs are folded once, the rest stay one row each. */
  const timelineEntries = createMemo(() =>
    groupActivitySections(groupedItems(), forkAfterGroupKey()),
  );

  /**
   * Key of the entry that holds the work of the running turn. It starts open so
   * the reader sees progress, and folds once the answer takes over the tail.
   */
  const liveEntryKey = createMemo(() =>
    status() === "running" && !nested() ? (timelineEntries().at(-1)?.key ?? null) : null,
  );

  function endsWithFork(entry: TimelineEntry): boolean {
    if (forkFrom() === null || forkAfterGroupKey() === null) return false;
    const last = entry.kind === "activity" ? entry.groups.at(-1) : entry;
    return last?.key === forkAfterGroupKey();
  }

  const sessionBusy = () =>
    status() === "running" || status() === "waiting_permission" || status() === "waiting_input";

  /**
   * A stopped session's agent process (and its terminals) is gone, even when the
   * last recorded terminal event was a turn-end snapshot.
   */
  const sessionStopped = createMemo(() => status() === "stopped" || status() === "error");

  /**
   * Latest message that can represent the current end of the conversation.
   * `last` scope only enables Fork there; `message` scope (future) enables all.
   */
  const lastForkableSeq = createMemo(() => {
    for (let index = props.items.length - 1; index >= 0; index -= 1) {
      const item = props.items[index];
      if (item.kind === "user") return item.seq;
      // Subagent messages are nested; only the parent conversation end is a
      // valid "last" fork point for the session.
      if (item.kind === "agent" && item.subagentId === null && !item.streaming) return item.seq;
    }
    return null;
  });

  const revertState = createMemo(() => {
    if (nested() || props.onrevert === undefined || !canRevert()) {
      return { canRevert: null as boolean | null, reason: null as string | null };
    }
    if (sessionBusy()) {
      return { canRevert: false, reason: m.chat_action_revert_running() as string | null };
    }
    return { canRevert: true, reason: null as string | null };
  });

  function forkState(item: TimelineItem): { canFork: boolean | null; reason: string | null } {
    if (nested() || props.onfork === undefined || forkScope() === null || forkScope() === "none") {
      return { canFork: null, reason: null };
    }
    if (sessionBusy() || (item.kind === "agent" && item.streaming)) {
      return { canFork: false, reason: m.chat_action_fork_running() };
    }
    if (forkScope() === "message") {
      return { canFork: true, reason: null };
    }
    return item.seq === lastForkableSeq()
      ? { canFork: true, reason: null }
      : { canFork: false, reason: m.chat_action_fork_latest() };
  }

  // Rows receive values, not accessors (CONVENTIONS: props are values). The
  // memo keeps the object stable during streaming: every field below only
  // changes when the value it carries changes.
  const context = createMemo<TimelineContext>(() => ({
    sessionId: props.sessionId,
    status: status(),
    subagentTitle: props.subagentTitle ?? null,
    onsubagent: props.onsubagent,
    onfork: props.onfork,
    onrevert: props.onrevert,
    onstopterminal: props.onstopterminal,
    stopTerminal,
    isStoppingTerminal,
    sessionStopped: sessionStopped(),
    lastForkableSeq: lastForkableSeq(),
    revert: revertState(),
    forkState,
    liveEntryKey: liveEntryKey(),
  }));

  return (
    <>
      {/* Spacing lives in `.timeline-list` (styles.css): blocks share one base
          gap, while a message group contributes its in-flow boundary row as the
          space below it. */}
      <ul class="timeline-list list-none p-0">
        <For each={timelineEntries()} keyed={(entry) => entry.key}>
          {(entry) => (
            <>
              <li>
                <TimelineRow entry={entry()} ctx={context()} />
              </li>
              <Show when={forkFrom() !== null && endsWithFork(entry())}>
                <li>
                  <ForkOrigin
                    parentSessionId={forkFrom() ?? ""}
                    title={props.forkFromTitle ?? null}
                  />
                </li>
              </Show>
            </>
          )}
        </For>

        <Show when={agentWorking() && !nested() && !tailIsLive()}>
          <li class="flex gap-2.5" role="status" aria-live="polite">
            <span class="bg-signal-soft text-signal mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-md">
              <IconBot class="size-3" />
            </span>
            <div class="bg-card text-muted-foreground border-border flex items-center gap-2 rounded-lg border px-3 py-1.5 text-[13px]">
              <IconLoaderCircle class="text-signal size-3.5 animate-spin" />
              <span>{m.chat_agent_working()}</span>
            </div>
          </li>
        </Show>
      </ul>

      <Show when={props.items.length === 0 && !agentWorking() && !nested()}>
        <p class="text-muted-foreground text-[13px]">{m.chat_empty_timeline()}</p>
      </Show>
    </>
  );
}
