<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { SearchHit } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { shortId } from "#lib/session-status.ts";
  import { connection, sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import SearchIcon from "@lucide/svelte/icons/search";

  let query = $state("");
  let hits = $state<SearchHit[]>([]);
  let searching = $state(false);
  let searched = $state(false);

  const roleLabel = $derived(connection.role === "hub" ? "hub.db" : "node.db");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const needle = query.trim();
    if (needle === "" || searching) return;
    searching = true;
    try {
      hits = await connection.client.search(needle, 50);
      searched = true;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      searching = false;
    }
  }

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ?? shortId(sessionId)
    );
  }

  /** Split an FTS5 snippet into segments around its `[match]` markers. */
  function segments(snippet: string): { text: string; match: boolean }[] {
    const parts: { text: string; match: boolean }[] = [];
    let rest = snippet;
    while (rest !== "") {
      const start = rest.indexOf("[");
      const end = start >= 0 ? rest.indexOf("]", start + 1) : -1;
      if (start < 0 || end < 0) {
        parts.push({ text: rest, match: false });
        break;
      }
      if (start > 0) parts.push({ text: rest.slice(0, start), match: false });
      parts.push({ text: rest.slice(start + 1, end), match: true });
      rest = rest.slice(end + 1);
    }
    return parts;
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
      {m.search_source_label()}
      <code class="bg-muted rounded px-1 py-0.5 font-mono text-[11px]">{roleLabel}</code>
    </p>
  </div>

  <form class="flex max-w-xl items-center gap-2" onsubmit={submit}>
    <Input
      bind:value={query}
      placeholder={m.search_placeholder()}
      aria-label={m.search_input_label()}
    />
    <Button type="submit" disabled={searching || query.trim() === ""}>
      <SearchIcon />
      {searching ? m.search_searching() : m.search_submit()}
    </Button>
  </form>

  {#if searched && hits.length === 0}
    <p class="text-muted-foreground text-[13px]">{m.search_empty()}</p>
  {:else if hits.length > 0}
    <ul class="border-border flex list-none flex-col border-t p-0">
      {#each hits as hit (hit.event_id)}
        <li class="border-border flex flex-col gap-1 border-b py-3">
          <div class="flex flex-wrap items-center gap-2 text-[11px]">
            <a class="text-[13px] font-medium hover:underline" href={`/sessions/${hit.session_id}`}>
              {sessionTitle(hit.session_id)}
            </a>
            <span class="text-muted-foreground desk-mono">#{hit.node_seq}</span>
            <span class="text-muted-foreground desk-mono">{hit.event_type}</span>
            <span class="text-muted-foreground desk-mono ml-auto">
              {formatRelativeTime(hit.created_at)}
            </span>
          </div>
          <p class="text-[13px] break-words whitespace-pre-wrap">
            {#each segments(hit.snippet) as segment}
              {#if segment.match}
                <mark class="bg-signal-soft text-ink rounded-sm px-0.5">{segment.text}</mark>
              {:else}{segment.text}{/if}
            {/each}
          </p>
        </li>
      {/each}
    </ul>
  {/if}
</div>
