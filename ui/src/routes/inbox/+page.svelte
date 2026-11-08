<script lang="ts">
  import ElicitationCard from "#lib/components/ElicitationCard.svelte";
  import PermissionCard from "#lib/components/PermissionCard.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { shortId } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  /** Sorted ascending so the oldest requests can be handled first. */
  const sorted = $derived(sync.inbox.toSorted((a, b) => a.created_at - b.created_at));
  const sortedElicitations = $derived(
    sync.inboxElicitations.toSorted((a, b) => a.created_at - b.created_at),
  );

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ?? shortId(sessionId)
    );
  }

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }
</script>

<div class="mx-auto flex max-w-5xl flex-col gap-4 p-4 md:p-6">
  <div class="flex items-center justify-between gap-2">
    <div>
      <h1 class="text-xl font-semibold">{m.inbox_title()}</h1>
      <p class="text-muted-foreground text-sm">
        {m.inbox_subtitle()}
      </p>
    </div>
    <Button variant="outline" size="sm" onclick={() => void sync.refreshInbox()}>
      <RefreshCwIcon />
      {m.common_refresh()}
    </Button>
  </div>

  <section class="flex flex-col gap-3">
    <h2 class="text-sm font-semibold">{m.inbox_approvals()}</h2>
    {#if sorted.length === 0}
      <p class="text-muted-foreground text-sm">{m.inbox_approvals_empty()}</p>
    {:else}
      <ul class="flex list-none flex-col gap-3 p-0">
        {#each sorted as request (request.request_id)}
          <li>
            <Card class="gap-0 p-0">
              <div
                class="bg-muted/40 flex flex-wrap items-center gap-x-3 gap-y-1 rounded-t-lg border-b px-3 py-2 text-xs"
              >
                <a href={`/sessions/${request.session_id}`} class="font-medium hover:underline">
                  {sessionTitle(request.session_id)}
                </a>
                <span class="text-muted-foreground">{nodeName(request.node_id)}</span>
                <span class="text-muted-foreground ml-auto">
                  {formatRelativeTime(request.created_at)}
                </span>
              </div>
              <div class="p-3">
                <PermissionCard
                  sessionId={request.session_id}
                  requestId={request.request_id}
                  toolName={request.tool_name}
                  summary={request.summary}
                  options={request.options}
                  details={request.details}
                  resolved={null}
                  createdAt={request.created_at}
                />
              </div>
            </Card>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="flex flex-col gap-3">
    <h2 class="text-sm font-semibold">{m.inbox_questions()}</h2>
    {#if sortedElicitations.length === 0}
      <p class="text-muted-foreground text-sm">{m.inbox_questions_empty()}</p>
    {:else}
      <ul class="flex list-none flex-col gap-3 p-0">
        {#each sortedElicitations as elicitation (elicitation.elicitation_id)}
          <li>
            <Card class="gap-0 p-0">
              <div
                class="bg-muted/40 flex flex-wrap items-center gap-x-3 gap-y-1 rounded-t-lg border-b px-3 py-2 text-xs"
              >
                <a href={`/sessions/${elicitation.session_id}`} class="font-medium hover:underline">
                  {sessionTitle(elicitation.session_id)}
                </a>
                <span class="text-muted-foreground">{nodeName(elicitation.node_id)}</span>
                <span class="text-muted-foreground ml-auto">
                  {formatRelativeTime(elicitation.created_at)}
                </span>
              </div>
              <div class="p-3">
                <ElicitationCard
                  sessionId={elicitation.session_id}
                  elicitationId={elicitation.elicitation_id}
                  message={elicitation.message}
                  requestedSchema={elicitation.requested_schema}
                  resolved={null}
                  createdAt={elicitation.created_at}
                />
              </div>
            </Card>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>
