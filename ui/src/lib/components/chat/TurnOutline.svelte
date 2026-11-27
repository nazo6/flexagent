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
  import { formatRelativeTime } from "#lib/format.ts";
  import type { SearchHit } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { snippetSegments } from "#lib/search.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { untrack } from "svelte";
  import ListIcon from "@lucide/svelte/icons/list";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import SearchIcon from "@lucide/svelte/icons/search";
  import XIcon from "@lucide/svelte/icons/x";

  /** In-session search page size and the server's upper bound. */
  const SEARCH_PAGE_SIZE = 50;
  const SEARCH_MAX = 200;

  interface Props {
    turns: readonly ChatTurn[];
    /** Session to search (in-session search runs on the full event log). */
    sessionId: string;
    /** Turn currently on screen (scroll spy, owned by the rail). */
    activeSeq?: number | null;
    onjump: (turn: ChatTurn) => void;
    /** Jump to a search hit (`node_seq`); the session page owns the scroll. */
    onjumpseq: (seq: number) => void;
    onclose: () => void;
  }

  let { turns, sessionId, activeSeq = null, onjump, onjumpseq, onclose }: Props = $props();

  let listEl = $state<HTMLDivElement | null>(null);
  let panelEl = $state<HTMLDivElement | null>(null);

  let query = $state("");
  let hits = $state<SearchHit[]>([]);
  let searching = $state(false);
  /** Query the current results belong to (null before the first search). */
  let searchedFor = $state<string | null>(null);
  let searchError = $state<string | null>(null);
  /** Current page size (grows with "Show more"). */
  let searchLimit = $state(SEARCH_PAGE_SIZE);
  /** Drops out-of-order responses when the query changes while a search runs. */
  let searchToken = 0;

  // A new query starts from the first page.
  $effect(() => {
    const needle = query;
    untrack(() => {
      if (needle !== "") searchLimit = SEARCH_PAGE_SIZE;
    });
  });

  $effect(() => {
    const needle = query.trim();
    const limit = searchLimit;
    if (needle === "") {
      searchToken += 1;
      hits = [];
      searchedFor = null;
      searchError = null;
      searching = false;
      return;
    }
    const token = ++searchToken;
    // Typing should not fire a request per keystroke.
    const timer = setTimeout(() => {
      void (async () => {
        searching = true;
        searchError = null;
        try {
          const result = await sync.connection.client.search(needle, limit, sessionId);
          if (token !== searchToken) return;
          hits = result;
          searchedFor = needle;
        } catch (error) {
          if (token !== searchToken) return;
          hits = [];
          searchedFor = needle;
          searchError = error instanceof Error ? error.message : String(error);
        } finally {
          if (token === searchToken) searching = false;
        }
      })();
    }, 250);
    return () => clearTimeout(timer);
  });

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
      <span class="text-muted-foreground desk-mono ml-auto text-[11px]">
        {query.trim() === "" ? turns.length : hits.length}
      </span>
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

    <div class="border-border flex shrink-0 items-center gap-1.5 border-b px-2 py-1.5">
      <SearchIcon class="text-muted-foreground size-3.5 shrink-0" />
      <input
        type="search"
        bind:value={query}
        placeholder={m.chat_turns_search_placeholder()}
        aria-label={m.chat_turns_search_placeholder()}
        class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-[12px] outline-none"
      />
      {#if query !== ""}
        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-5 shrink-0 items-center justify-center rounded-md"
          title={m.chat_turns_search_clear()}
          aria-label={m.chat_turns_search_clear()}
          onclick={() => (query = "")}
        >
          <XIcon class="size-3" />
        </button>
      {/if}
    </div>

    <div bind:this={listEl} class="min-h-0 flex-1 overflow-y-auto p-1">
      {#if query.trim() !== ""}
        {#if searching}
          <p class="text-muted-foreground flex items-center gap-2 p-2 text-[12px]">
            <LoaderCircleIcon class="size-3.5 animate-spin" />
            {m.chat_turns_search_loading()}
          </p>
        {:else if searchError !== null}
          <p class="text-destructive p-2 text-[12px]">{m.chat_turns_search_failed()}</p>
        {:else if searchedFor !== null && hits.length === 0}
          <p class="text-muted-foreground p-2 text-[12px]">{m.chat_turns_search_empty()}</p>
        {:else}
          <ul class="m-0 flex list-none flex-col gap-0.5 p-0">
            {#each hits as hit (hit.event_id)}
              <li>
                <button
                  type="button"
                  data-search-hit={hit.node_seq}
                  class="hover:bg-accent flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors"
                  onclick={() => {
                    onjumpseq(hit.node_seq);
                    onclose();
                  }}
                >
                  <span
                    class="text-muted-foreground desk-mono mt-0.5 w-8 shrink-0 text-right text-[10px]"
                  >
                    #{hit.node_seq}
                  </span>
                  <span class="min-w-0 flex-1">
                    <span class="line-clamp-2 block text-[12px] leading-snug">
                      {#each snippetSegments(hit.snippet) as segment, segmentIndex (segmentIndex)}
                        {#if segment.match}
                          <mark class="bg-signal-soft text-ink rounded-sm px-0.5"
                            >{segment.text}</mark
                          >
                        {:else}{segment.text}{/if}
                      {/each}
                    </span>
                    <span
                      class="text-muted-foreground mt-0.5 flex items-center gap-1.5 text-[10px]"
                    >
                      <span class="desk-mono">{hit.event_type}</span>
                      <span class="ml-auto">{formatRelativeTime(hit.created_at)}</span>
                    </span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
          {#if hits.length >= searchLimit && searchLimit < SEARCH_MAX}
            <div class="flex justify-center py-1">
              <button
                type="button"
                class="text-muted-foreground hover:bg-accent hover:text-foreground rounded-md px-2 py-1 text-[11px] disabled:opacity-50"
                disabled={searching}
                onclick={() => (searchLimit += SEARCH_PAGE_SIZE)}
              >
                {m.chat_turns_search_more()}
              </button>
            </div>
          {/if}
        {/if}
      {:else}
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
                          class={cn(
                            "rounded-full border px-1.5 text-[10px]",
                            badgeClass[badge.tone],
                          )}
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
      {/if}
    </div>
  </div>
</div>
