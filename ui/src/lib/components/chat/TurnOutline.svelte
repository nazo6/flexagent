<script lang="ts">
  /**
   * Table of contents for the conversation: one row per turn, with the prompt
   * so the reader can pick a turn by content instead of by position.
   *
   * It is an overlay anchored next to the rail, not a docked panel: on wide
   * screens the workspace dock already competes for the right-hand width.
   */
  import type { ChatTurn } from "#lib/chat-turns.ts";
  import { turnToolLabel } from "#lib/chat-turns.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import ListIcon from "@lucide/svelte/icons/list";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    turns: readonly ChatTurn[];
    /** Turn currently on screen (scroll spy, owned by the rail). */
    activeSeq?: number | null;
    onjump: (turn: ChatTurn) => void;
    onclose: () => void;
  }

  let { turns, activeSeq = null, onjump, onclose }: Props = $props();

  let listEl = $state<HTMLDivElement | null>(null);
  let panelEl = $state<HTMLDivElement | null>(null);

  interface Badge {
    label: string;
    tone: "danger" | "warn" | "neutral";
  }

  function badgesOf(turn: ChatTurn): Badge[] {
    const badges: Badge[] = [];
    if (turn.attention.approval) badges.push({ label: m.chat_turns_approval(), tone: "danger" });
    if (turn.attention.question) badges.push({ label: m.chat_turns_question(), tone: "danger" });
    if (turn.attention.error) badges.push({ label: m.chat_turns_error(), tone: "warn" });
    if (turn.tools > 0) {
      badges.push({ label: turnToolLabel(turn.tools), tone: "neutral" });
    }
    return badges;
  }

  const badgeClass: Record<Badge["tone"], string> = {
    danger: "border-danger/30 bg-danger-soft text-danger",
    warn: "border-warn/30 bg-warn-soft text-warn",
    neutral: "border-border bg-chip text-muted-foreground",
  };

  $effect(() => {
    panelEl?.focus({ preventScroll: true });
  });

  // Keep the current turn visible while the reader scrolls the chat.
  $effect(() => {
    const seq = activeSeq;
    const list = listEl;
    if (seq === null || list === null) return;
    const row = list.querySelector<HTMLElement>(`[data-turn-row="${seq}"]`);
    if (row === null) return;
    const top = row.offsetTop;
    const bottom = top + row.offsetHeight;
    if (top < list.scrollTop || bottom > list.scrollTop + list.clientHeight) {
      list.scrollTop = Math.max(0, top - 8);
    }
  });
</script>

<div class="absolute inset-0 z-20">
  <button
    type="button"
    class="absolute inset-0 cursor-default"
    aria-label={m.chat_turns_close()}
    onclick={onclose}
  ></button>

  <div
    bind:this={panelEl}
    class="bg-popover border-border absolute top-3 right-8 bottom-3 flex w-[19rem] max-w-[calc(100%-2.5rem)] flex-col overflow-hidden rounded-xl border shadow-lg outline-none"
    role="dialog"
    aria-label={m.chat_turns_title()}
    tabindex="-1"
    onkeydown={(event) => {
      if (event.key === "Escape") onclose();
    }}
  >
    <div class="border-border flex shrink-0 items-center gap-2 border-b px-3 py-2">
      <ListIcon class="text-muted-foreground size-4 shrink-0" />
      <span class="text-[13px] font-semibold">{m.chat_turns_title()}</span>
      <span class="text-muted-foreground desk-mono ml-auto text-[11px]">{turns.length}</span>
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-6 items-center justify-center rounded-md"
        title={m.chat_turns_close()}
        aria-label={m.chat_turns_close()}
        onclick={onclose}
      >
        <XIcon class="size-3.5" />
      </button>
    </div>

    <div bind:this={listEl} class="min-h-0 flex-1 overflow-y-auto p-1">
      <ul class="m-0 flex list-none flex-col gap-0.5 p-0">
        {#each turns as turn, index (turn.seq)}
          {@const badges = badgesOf(turn)}
          <li>
            <button
              type="button"
              data-turn-row={turn.seq}
              aria-current={turn.seq === activeSeq ? "true" : undefined}
              class={cn(
                "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
                turn.seq === activeSeq ? "bg-signal-soft" : "hover:bg-accent",
              )}
              onclick={() => onjump(turn)}
            >
              <span
                class="text-muted-foreground desk-mono mt-0.5 w-4 shrink-0 text-right text-[10px]"
              >
                {String(index + 1).padStart(2, "0")}
              </span>
              <span class="min-w-0 flex-1">
                <span class="line-clamp-2 block text-[12px] leading-snug">
                  {turn.title === "" ? m.chat_turns_untitled() : turn.title}
                </span>
                {#if badges.length > 0}
                  <span class="mt-1 flex flex-wrap items-center gap-1">
                    {#each badges as badge (badge.label)}
                      <span
                        class={cn("rounded-full border px-1.5 text-[10px]", badgeClass[badge.tone])}
                      >
                        {badge.label}
                      </span>
                    {/each}
                  </span>
                {/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    </div>
  </div>
</div>
