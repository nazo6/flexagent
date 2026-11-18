import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { formatRelativeTime } from "#lib/format.ts";
import type { SearchHit } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { shortId } from "#lib/session-status.ts";
import { connection, sync } from "#lib/stores/app.ts";
import IconSearch from "~icons/lucide/search";

/** A piece of an FTS5 snippet, marked when it is the match. */
interface Segment {
  text: string;
  match: boolean;
}

export default function SearchPage() {
  const [query, setQuery] = createSignal("");
  const [hits, setHits] = createSignal<SearchHit[]>([]);
  const [searching, setSearching] = createSignal(false);
  const [searched, setSearched] = createSignal(false);

  const scopeLabel = createMemo(() =>
    connection.role === "hub" ? m.search_scope_hub() : m.search_scope_node(),
  );

  createEffect(
    () => m.search_page_title(),
    (title) => {
      document.title = title;
    },
  );

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const needle = query().trim();
    if (needle === "" || searching()) return;
    setSearching(true);
    try {
      setHits(await connection.client.search(needle, 50));
      setSearched(true);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSearching(false);
    }
  }

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ?? shortId(sessionId)
    );
  }

  /** Split an FTS5 snippet into segments around its `[match]` markers. */
  function segments(snippet: string): Segment[] {
    const parts: Segment[] = [];
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

  return (
    <div class="desk-page">
      <div class="flex flex-col gap-1">
        <h1 class="text-xl font-semibold tracking-tight">{m.search_title()}</h1>
        <p class="text-muted-foreground text-[13px]">
          {m.search_description()} {scopeLabel()}
        </p>
      </div>

      <form class="flex max-w-xl items-center gap-2" onSubmit={submit}>
        <div class="relative min-w-0 flex-1">
          <IconSearch class="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4" />
          <Input
            value={query()}
            onValueChange={setQuery}
            placeholder={m.search_placeholder()}
            aria-label={m.search_input_label()}
            class="pl-9"
          />
        </div>
        <Button type="submit" disabled={searching() || query().trim() === ""}>
          <IconSearch />
          {searching() ? m.search_searching() : m.search_submit()}
        </Button>
      </form>

      <Show when={searched() && hits().length === 0}>
        <p class="text-muted-foreground text-[13px]">{m.search_empty()}</p>
      </Show>
      <Show when={hits().length > 0}>
        <ul class="border-border flex list-none flex-col border-t p-0">
          <For each={hits()}>
            {(hit) => (
              <li class="border-border flex flex-col gap-1 border-b py-3">
                <div class="flex flex-wrap items-center gap-2 text-[11px]">
                  <a
                    class="text-[13px] font-medium hover:underline"
                    href={`/sessions/${hit.session_id}`}
                  >
                    {sessionTitle(hit.session_id)}
                  </a>
                  <span class="text-muted-foreground desk-mono">#{hit.node_seq}</span>
                  <span class="text-muted-foreground desk-mono">{hit.event_type}</span>
                  <span class="text-muted-foreground desk-mono ml-auto">
                    {formatRelativeTime(hit.created_at)}
                  </span>
                </div>
                <p class="text-[13px] break-words whitespace-pre-wrap">
                  <For each={segments(hit.snippet)}>
                    {(segment) =>
                      segment.match ? (
                        <mark class="bg-signal-soft text-ink rounded-sm px-0.5">
                          {segment.text}
                        </mark>
                      ) : (
                        segment.text
                      )
                    }
                  </For>
                </p>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
