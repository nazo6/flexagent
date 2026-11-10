<script lang="ts">
  import ElicitationCard from "#lib/components/ElicitationCard.svelte";
  import PermissionCard from "#lib/components/PermissionCard.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
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

<svelte:head>
  <title>{m.inbox_page_title()}</title>
</svelte:head>

<div class="desk-page">
  <div class="flex flex-wrap items-end justify-between gap-3">
    <div class="flex flex-col gap-1">
      <h1 class="text-xl font-semibold tracking-tight">{m.inbox_title()}</h1>
      <p class="text-muted-foreground text-[13px]">
        {m.inbox_subtitle()}
      </p>
    </div>
    <Button variant="outline" size="sm" onclick={() => void sync.refreshInbox()}>
      <RefreshCwIcon />
      {m.common_refresh()}
    </Button>
  </div>

  {#if sync.inboxLoading && sorted.length === 0 && sortedElicitations.length === 0}
    <p class="text-muted-foreground text-sm">{m.common_loading()}</p>
  {:else if sync.inboxError !== null && sorted.length === 0 && sortedElicitations.length === 0}
    <div
      class="border-danger/30 bg-danger-soft/50 flex flex-col items-start gap-2 rounded-lg border p-3"
    >
      <p class="text-danger text-[13px]">
        {m.common_load_failed()}
        {sync.inboxError}
      </p>
      <Button variant="outline" size="sm" onclick={() => void sync.refreshInbox()}>
        <RefreshCwIcon />
        {m.common_retry()}
      </Button>
    </div>
  {:else}
    <section class="flex flex-col gap-2">
      <div class="flex items-center gap-2">
        <h2 class="desk-label">{m.inbox_approvals()}</h2>
        <span class="text-muted-foreground desk-mono text-[11px]">{sorted.length}</span>
      </div>
      {#if sorted.length === 0}
        <p class="text-muted-foreground text-[13px]">{m.inbox_approvals_empty()}</p>
      {:else}
        <ul class="border-border flex list-none flex-col border-t p-0">
          {#each sorted as request (request.request_id)}
            <li class="border-border border-b py-4">
              <div class="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
                <a
                  href={`/sessions/${request.session_id}`}
                  class="text-[13px] font-medium hover:underline"
                >
                  {sessionTitle(request.session_id)}
                </a>
                <span class="text-muted-foreground desk-mono">{nodeName(request.node_id)}</span>
                <span class="text-muted-foreground desk-mono ml-auto">
                  {formatRelativeTime(request.created_at)}
                </span>
              </div>
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
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="flex flex-col gap-2">
      <div class="flex items-center gap-2">
        <h2 class="desk-label">{m.inbox_questions()}</h2>
        <span class="text-muted-foreground desk-mono text-[11px]">{sortedElicitations.length}</span>
      </div>
      {#if sortedElicitations.length === 0}
        <p class="text-muted-foreground text-[13px]">{m.inbox_questions_empty()}</p>
      {:else}
        <ul class="border-border flex list-none flex-col border-t p-0">
          {#each sortedElicitations as elicitation (elicitation.elicitation_id)}
            <li class="border-border border-b py-4">
              <div class="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
                <a
                  href={`/sessions/${elicitation.session_id}`}
                  class="text-[13px] font-medium hover:underline"
                >
                  {sessionTitle(elicitation.session_id)}
                </a>
                <span class="text-muted-foreground desk-mono">{nodeName(elicitation.node_id)}</span>
                <span class="text-muted-foreground desk-mono ml-auto">
                  {formatRelativeTime(elicitation.created_at)}
                </span>
              </div>
              <ElicitationCard
                sessionId={elicitation.session_id}
                elicitationId={elicitation.elicitation_id}
                message={elicitation.message}
                requestedSchema={elicitation.requested_schema}
                resolved={null}
                createdAt={elicitation.created_at}
              />
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
</div>
