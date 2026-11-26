<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { SearchHit } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { snippetSegments } from "#lib/search.ts";
  import { shortId } from "#lib/session-status.ts";
  import { connection, sync } from "#lib/stores/app.svelte.ts";
  import { motion } from "#lib/motion.svelte.ts";
  import { toast } from "svelte-sonner";
  import { fly } from "svelte/transition";
  import SearchIcon from "@lucide/svelte/icons/search";

  /** Results per page; the API clamps the total at 500. */
  const SEARCH_PAGE_SIZE = 50;
  const SEARCH_MAX = 500;

  let query = $state("");
  let hits = $state<SearchHit[]>([]);
  let searching = $state(false);
  let searched = $state(false);
  /** Current page size (grows with "Show more"; the API caps at 500). */
  let limit = $state(SEARCH_PAGE_SIZE);
  /** Query whose results are on screen (for loading more). */
  let lastQuery = $state("");

  const canShowMore = $derived(hits.length >= limit && limit < SEARCH_MAX);

  const scopeLabel = $derived(
    connection.role === "hub" ? m.search_scope_hub() : m.search_scope_node(),
  );

  async function runSearch(needle: string, nextLimit: number): Promise<void> {
    if (searching) return;
    searching = true;
    try {
      hits = await connection.client.search(needle, nextLimit);
      searched = true;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      searching = false;
    }
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const needle = query.trim();
    if (needle === "") return;
    limit = SEARCH_PAGE_SIZE;
    lastQuery = needle;
    await runSearch(needle, limit);
  }

  async function loadMore(): Promise<void> {
    if (!canShowMore || searching) return;
    limit += SEARCH_PAGE_SIZE;
    await runSearch(lastQuery, limit);
  }

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ?? shortId(sessionId)
    );
  }

  /** Project / node label per hit (the sidebar list is paginated, so both may be missing). */
  const hitMeta = $derived.by(() => {
    const meta = new Map<string, { project: string | null; node: string | null }>();
    for (const hit of hits) {
      if (meta.has(hit.session_id)) continue;
      const session = sync.sessions.find((entry) => entry.session_id === hit.session_id);
      const project =
        session === undefined
          ? null
          : (sync.projects.find((entry) => entry.project_id === session.project_id)?.name ?? null);
      const node =
        session === undefined
          ? null
          : (sync.nodes.find((entry) => entry.node_id === session.node_id)?.name ?? null);
      meta.set(hit.session_id, { project, node });
    }
    return meta;
  });

  /** Deep link to the matched event (the session page loads and flashes it). */
  function hitHref(hit: SearchHit): string {
    return `/sessions/${hit.session_id}?seq=${hit.node_seq}`;
  }
</script>

<svelte:head>
  <title>{m.search_page_title()}</title>
</svelte:head>

<div class="desk-page">
  <div class="flex flex-col gap-1">
    <h1 class="text-xl font-semibold tracking-tight">{m.search_title()}</h1>
    <p class="text-muted-foreground text-[13px]">
      {m.search_description()}
      {scopeLabel}
    </p>
  </div>

  <form class="flex max-w-xl items-center gap-2" onsubmit={submit}>
    <div class="relative min-w-0 flex-1">
      <SearchIcon
        class="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4"
      />
      <Input
        bind:value={query}
        placeholder={m.search_placeholder()}
        aria-label={m.search_input_label()}
        class="pl-9"
      />
    </div>
    <Button type="submit" disabled={searching || query.trim() === ""}>
      <SearchIcon />
      {searching ? m.search_searching() : m.search_submit()}
    </Button>
  </form>

  {#if searched && hits.length === 0}
    <div class="flex flex-col gap-1">
      <p class="text-[13px]">{m.search_empty_for({ query: lastQuery })}</p>
      <p class="text-muted-foreground text-[12px]">{m.search_empty_hint()}</p>
    </div>
  {:else if hits.length > 0}
    <p class="text-muted-foreground text-[12px]">{m.search_result_count({ count: hits.length })}</p>
    <ul class="border-border flex list-none flex-col border-t p-0">
      {#each hits as hit, index (hit.event_id)}
        {@const meta = hitMeta.get(hit.session_id)}
        <li
          in:fly|local={{
            y: 4,
            duration: motion.duration(140),
            delay: motion.reduced ? 0 : Math.min(index, 5) * 15,
          }}
          class="border-border flex flex-col gap-1 border-b py-3"
        >
          <div class="flex flex-wrap items-center gap-2 text-[11px]">
            <a class="text-[13px] font-medium hover:underline" href={hitHref(hit)}>
              {sessionTitle(hit.session_id)}
            </a>
            {#if meta?.project}
              <span class="text-muted-foreground max-w-[16rem] truncate">{meta.project}</span>
            {/if}
            {#if meta?.node}
              <span class="text-muted-foreground max-w-[12rem] truncate">{meta.node}</span>
            {/if}
            <span class="text-muted-foreground desk-mono">#{hit.node_seq}</span>
            <span class="text-muted-foreground desk-mono">{hit.event_type}</span>
            <span class="text-muted-foreground desk-mono ml-auto">
              {formatRelativeTime(hit.created_at)}
            </span>
          </div>
          <a href={hitHref(hit)} class="block transition-opacity hover:opacity-80">
            <p class="text-[13px] break-words whitespace-pre-wrap">
              {#each snippetSegments(hit.snippet) as segment}
                {#if segment.match}
                  <mark class="bg-signal-soft text-ink rounded-sm px-0.5">{segment.text}</mark>
                {:else}{segment.text}{/if}
              {/each}
            </p>
          </a>
        </li>
      {/each}
    </ul>
    {#if canShowMore}
      <div class="flex justify-center pt-1">
        <Button
          variant="outline"
          size="sm"
          class="h-7 px-2 text-[12px]"
          disabled={searching}
          onclick={() => void loadMore()}
        >
          {searching ? m.search_searching() : m.search_more_results()}
        </Button>
      </div>
    {/if}
  {/if}
</div>
