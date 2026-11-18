import { createEffect, createMemo, createSignal, Show } from "solid-js";
import MarkdownText from "#lib/components/MarkdownText.tsx";
import { m } from "#lib/paraglide/messages.js";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import IconBrain from "~icons/lucide/brain";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronRight from "~icons/lucide/chevron-right";

type ThoughtItem = Extract<TimelineItem, { kind: "thought" }>;

export interface ThoughtBlockProps {
  item: ThoughtItem;
}

/**
 * Reasoning block.
 *
 * It opens by itself while the agent is thinking and folds away once the
 * finalized `agent_thought` event arrives, unless the reader toggled it (same
 * idiom as `ToolCallGroup`). The body scrolls on its own so a long reasoning
 * stream does not push the rest of the conversation around.
 */
export default function ThoughtBlock(props: ThoughtBlockProps) {
  const [userToggled, setUserToggled] = createSignal(false);
  const [userIsOpen, setUserIsOpen] = createSignal(false);
  // A derived boolean (rather than reading `item.streaming` in the effect)
  // keeps the ticker from restarting whenever a delta replaces the item object.
  const streaming = () => props.item.streaming;
  const isOpen = () => (userToggled() ? userIsOpen() : streaming());

  /** Ticks while streaming so the elapsed time in the summary stays current. */
  const [now, setNow] = createSignal(Date.now());
  createEffect(
    () => streaming(),
    (active) => {
      if (!active) return;
      setNow(Date.now());
      const timer = setInterval(() => setNow(Date.now()), 500);
      return () => clearInterval(timer);
    },
  );

  /** Elapsed reasoning time (null when replayed from history without live deltas). */
  const seconds = createMemo(() => {
    if (props.item.startedAt === null) return null;
    const end = props.item.endedAt ?? (streaming() ? now() : props.item.startedAt);
    return Math.max(0, Math.round((end - props.item.startedAt) / 1000));
  });
  const duration = createMemo(() => {
    const value = seconds();
    if (value === null) return null;
    if (value < 60) return m.chat_thought_seconds({ seconds: value });
    return m.chat_thought_minutes({ minutes: Math.floor(value / 60), seconds: value % 60 });
  });

  /** First non-empty line, shown in the collapsed summary. */
  const preview = createMemo(() => {
    if (isOpen()) return null;
    const line = props.item.text
      .split("\n")
      .find((candidate) => candidate.trim() !== "")
      ?.trim();
    if (line === undefined) return null;
    // The summary is plain text, so drop the inline emphasis markers.
    const plain = line
      .replace(/[*_`#>]/g, "")
      .replace(/^[-+]\s+/, "")
      .trim();
    return plain === "" ? null : plain;
  });

  const [body, setBody] = createSignal<HTMLDivElement>();

  // Keep the reasoning body pinned to its own bottom while it streams in.
  createEffect(
    () => ({ element: body(), text: props.item.text, streaming: streaming() }),
    ({ element, text, streaming: active }) => {
      if (element === undefined || !active || text === "") return;
      element.scrollTop = element.scrollHeight;
    },
  );

  function toggle(): void {
    setUserToggled(true);
    setUserIsOpen(!isOpen());
  }

  return (
    <div>
      <button
        type="button"
        class="text-muted-foreground hover:text-foreground flex w-full items-center gap-1.5 text-[13px] select-none"
        aria-expanded={isOpen() ? "true" : "false"}
        onClick={toggle}
      >
        {isOpen() ? (
          <IconChevronDown class="size-3.5 shrink-0" />
        ) : (
          <IconChevronRight class="size-3.5 shrink-0" />
        )}
        <IconBrain class="size-3.5 shrink-0" />
        <span class="shrink-0 font-medium">
          {streaming() ? m.chat_thought_streaming() : m.chat_thought_process()}
        </span>
        <Show when={duration()}>
          {(value) => (
            <span class="text-muted-foreground/70 shrink-0 text-[11px] tabular-nums">
              {value()}
            </span>
          )}
        </Show>
        <Show
          when={streaming()}
          fallback={
            <Show when={preview()}>
              {(text) => (
                <span class="text-muted-foreground/60 min-w-0 flex-1 truncate text-left">
                  {text()}
                </span>
              )}
            </Show>
          }
        >
          <span class="text-signal shrink-0 animate-pulse">…</span>
        </Show>
      </button>

      <Show when={isOpen()}>
        <div
          ref={setBody}
          class="text-muted-foreground border-line mt-1.5 max-h-64 overflow-y-auto rounded-md border p-2.5 text-[13px] leading-relaxed"
        >
          <MarkdownText text={props.item.text} streaming={streaming()} />
        </div>
      </Show>
    </div>
  );
}
