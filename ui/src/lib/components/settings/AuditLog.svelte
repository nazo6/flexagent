<script lang="ts">
  import { onMount } from 'svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import { formatEpochMs } from '$lib/format';
  import type { AuditLogEntry } from '$lib/generated/AuditLogEntry';
  import { shortId } from '$lib/session-status';
  import { connection, sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  let logs = $state<AuditLogEntry[]>([]);
  let loading = $state(false);

  const roleLabel = $derived(connection.role === 'central_server' ? 'server.db' : 'node.db');

  async function load() {
    loading = true;
    try {
      logs = await connection.client.auditLogs(100);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ??
      shortId(sessionId)
    );
  }

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }

  const ACTION_LABELS: Record<string, string> = {
    session_start: 'セッション開始',
    permission_resolved: '承認解決',
    pty_spawn: 'PTY 起動',
    kill_switch: '緊急停止',
    worktree_manage: 'Worktree 操作',
    session_resume: 'セッション再開'
  };
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <p class="text-muted-foreground text-sm">記録先: <code>{roleLabel}</code> (新しい順)</p>
    <Button variant="outline" size="sm" disabled={loading} onclick={() => void load()}>
      <RefreshCwIcon />
      再読込
    </Button>
  </div>

  {#if logs.length === 0}
    <p class="text-muted-foreground text-sm">
      {loading ? '読み込み中…' : '監査ログはまだありません。'}
    </p>
  {:else}
    <ul class="flex list-none flex-col gap-2 p-0">
      {#each logs as log (log.id)}
        <li>
          <Card class="gap-1.5 p-3">
            <div class="flex flex-wrap items-center gap-2 text-xs">
              <Badge variant="outline">{ACTION_LABELS[log.action] ?? log.action}</Badge>
              <span class="text-muted-foreground">{formatEpochMs(log.created_at)}</span>
              <span class="text-muted-foreground">{log.client_ip}</span>
              <span class="text-muted-foreground">{log.auth_subject}</span>
              {#if log.node_id}
                <Badge variant="secondary">{nodeName(log.node_id)}</Badge>
              {/if}
              {#if log.session_id}
                <a
                  class="ml-auto text-xs font-medium hover:underline"
                  href={`/sessions/${log.session_id}`}
                >
                  {sessionTitle(log.session_id)}
                </a>
              {/if}
            </div>
            {#if log.client_user_agent}
              <p class="text-muted-foreground truncate text-[11px]">{log.client_user_agent}</p>
            {/if}
            {#if log.details !== null && log.details !== undefined}
              <details>
                <summary
                  class="text-muted-foreground hover:text-foreground cursor-pointer text-xs select-none"
                >
                  詳細
                </summary>
                <pre
                  class="bg-muted/40 mt-1 max-h-40 overflow-auto rounded-md p-2 font-mono text-xs whitespace-pre-wrap">{JSON.stringify(
                    log.details,
                    null,
                    2
                  )}</pre>
              </details>
            {/if}
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>
