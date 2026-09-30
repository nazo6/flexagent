<script lang="ts">
  import SessionStatusBadge from '$lib/components/SessionStatusBadge.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import { formatRelativeTime } from '$lib/format';
  import { shortId } from '$lib/session-status';
  import { sync } from '$lib/stores/app.svelte';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }
</script>

<div class="flex flex-col gap-4">
  <div class="flex items-center justify-between gap-2">
    <h1 class="text-xl font-semibold">セッション</h1>
    <Button variant="outline" size="sm" onclick={() => void sync.refreshAll()}>
      <RefreshCwIcon />
      再読込
    </Button>
  </div>

  {#if sync.sessions.length === 0}
    <p class="text-muted-foreground text-sm">
      まだセッションがありません。「プロジェクト」画面から新しいセッションを起動できます。
    </p>
  {:else}
    <ul class="grid list-none grid-cols-1 gap-3 p-0 sm:grid-cols-2 lg:grid-cols-3">
      {#each sync.sessions as session (session.session_id)}
        <li>
          <a href={`/sessions/${session.session_id}`} class="block h-full">
            <Card class="hover:border-primary/50 h-full gap-0 p-4 transition-colors">
              <div class="flex items-start justify-between gap-2">
                <span class="line-clamp-2 text-sm font-medium">{session.title}</span>
                <SessionStatusBadge status={session.status} />
              </div>
              <div class="text-muted-foreground mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs">
                <span>{nodeName(session.node_id)}</span>
                <span class="truncate">{session.project_id}</span>
                {#if session.git_branch}
                  <span>branch: {session.git_branch}</span>
                {/if}
                <span>{session.agent_id}</span>
              </div>
              <div class="text-muted-foreground mt-1 text-xs">
                更新: {formatRelativeTime(session.updated_at)}
              </div>
            </Card>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</div>
