<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import { Input } from '$lib/components/ui/input';
  import { formatRelativeTime } from '$lib/format';
  import type { SearchHit } from '$lib/generated/SearchHit';
  import { shortId } from '$lib/session-status';
  import { connection, sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import SearchIcon from '@lucide/svelte/icons/search';

  let query = $state('');
  let hits = $state<SearchHit[]>([]);
  let searching = $state(false);
  let searched = $state(false);

  const roleLabel = $derived(connection.role === 'central_server' ? 'server.db' : 'node.db');

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const needle = query.trim();
    if (needle === '' || searching) return;
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
      sync.sessions.find((session) => session.session_id === sessionId)?.title ??
      shortId(sessionId)
    );
  }

  /** FTS5 snippet の `[match]` マーカーをセグメントへ分割する。 */
  function segments(snippet: string): { text: string; match: boolean }[] {
    const parts: { text: string; match: boolean }[] = [];
    let rest = snippet;
    while (rest !== '') {
      const start = rest.indexOf('[');
      const end = start >= 0 ? rest.indexOf(']', start + 1) : -1;
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

<div class="mx-auto flex max-w-5xl flex-col gap-4 p-4 md:p-6">
  <div>
    <h1 class="text-xl font-semibold">全文検索</h1>
    <p class="text-muted-foreground text-sm">
      セッションイベントの全文検索 (FTS5 trigram / 記録先: <code>{roleLabel}</code>)
    </p>
  </div>

  <form class="flex max-w-xl items-center gap-2" onsubmit={submit}>
    <Input
      bind:value={query}
      placeholder="検索語 (3文字以上で高速、2文字以下は部分一致フォールバック)"
    />
    <Button type="submit" disabled={searching || query.trim() === ''}>
      <SearchIcon />
      {searching ? '検索中…' : '検索'}
    </Button>
  </form>

  {#if searched && hits.length === 0}
    <p class="text-muted-foreground text-sm">該当するイベントはありませんでした。</p>
  {:else if hits.length > 0}
    <ul class="flex list-none flex-col gap-2 p-0">
      {#each hits as hit (hit.event_id)}
        <li>
          <Card class="gap-1.5 p-3">
            <div class="flex flex-wrap items-center gap-2 text-xs">
              <a class="font-medium hover:underline" href={`/sessions/${hit.session_id}`}>
                {sessionTitle(hit.session_id)}
              </a>
              <span class="text-muted-foreground font-mono">#{hit.node_seq}</span>
              <span class="text-muted-foreground">{hit.event_type}</span>
              <span class="text-muted-foreground ml-auto">
                {formatRelativeTime(hit.created_at)}
              </span>
            </div>
            <p class="text-sm break-words whitespace-pre-wrap">
              {#each segments(hit.snippet) as segment}
                {#if segment.match}
                  <mark class="bg-yellow-200 text-inherit dark:bg-yellow-800">{segment.text}</mark>
                {:else}{segment.text}{/if}
              {/each}
            </p>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>
