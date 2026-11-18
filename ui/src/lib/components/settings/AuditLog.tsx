import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { formatEpochMs } from "#lib/format.ts";
import type { AuditLogEntry } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { shortId } from "#lib/session-status.ts";
import { connection, sync } from "#lib/stores/app.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";

/** Audit log of the endpoint or of a target node. */
export default function AuditLog(props: {
  /** Target node (`""` = the endpoint itself, i.e. the hub's own audit log). */
  nodeId?: string;
}) {
  const [logs, setLogs] = createSignal<AuditLogEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  let generation = 0;

  const roleLabel = () =>
    (props.nodeId ?? "") !== "" || connection.role !== "hub" ? "node.db" : "hub.db";

  async function load(): Promise<void> {
    const current = ++generation;
    setLoading(true);
    try {
      const nodeId = props.nodeId ?? "";
      const next =
        nodeId === ""
          ? await connection.client.auditLogs(100)
          : await connection.client.nodeAuditLogs(nodeId, 100);
      if (current !== generation) return;
      setLogs(next);
    } catch (error) {
      if (current === generation) toast.error(errorMessage(error));
    } finally {
      if (current === generation) setLoading(false);
    }
  }

  createEffect(
    () => props.nodeId ?? "",
    () => {
      setLogs([]);
      void load();
    },
  );

  function sessionTitle(sessionId: string): string {
    return (
      sync.sessions.find((session) => session.session_id === sessionId)?.title ?? shortId(sessionId)
    );
  }

  function nodeName(id: string): string {
    return sync.nodes.find((node) => node.node_id === id)?.name ?? shortId(id);
  }

  const actionLabels = createMemo<Record<string, string>>(() => ({
    session_start: m.settings_audit_action_session_start(),
    permission_resolved: m.settings_audit_action_permission_resolved(),
    elicitation_resolved: m.settings_audit_action_elicitation_resolved(),
    pty_spawn: m.settings_audit_action_pty_spawn(),
    kill_switch: m.settings_audit_action_kill_switch(),
    node_shutdown: m.settings_audit_action_node_shutdown(),
    workspace_git_commit: m.settings_audit_action_workspace_git_commit(),
    workspace_git_pull: m.settings_audit_action_workspace_git_pull(),
    workspace_git_push: m.settings_audit_action_workspace_git_push(),
    worktree_manage: m.settings_audit_action_worktree_manage(),
    session_resume: m.settings_audit_action_session_resume(),
  }));

  return (
    <div class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <p class="text-muted-foreground text-sm">
          {m.settings_audit_source()} <code>{roleLabel()}</code> {m.settings_audit_newest_first()}
        </p>
        <Button variant="outline" size="sm" disabled={loading()} onClick={() => void load()}>
          <IconRefreshCw />
          {m.common_refresh()}
        </Button>
      </div>

      <Show
        when={logs().length !== 0}
        fallback={
          <p class="text-muted-foreground text-sm">
            {loading() ? m.common_loading() : m.settings_audit_empty()}
          </p>
        }
      >
        <ul class="flex list-none flex-col gap-2 p-0">
          <For each={logs()}>
            {(log) => (
              <li>
                <Card class="gap-1.5 p-3">
                  <div class="flex flex-wrap items-center gap-2 text-[13px]">
                    <Badge variant="outline">{actionLabels()[log.action] ?? log.action}</Badge>
                    <span class="text-muted-foreground">{formatEpochMs(log.created_at)}</span>
                    <span class="text-muted-foreground">{log.client_ip}</span>
                    <span class="text-muted-foreground">{log.auth_subject}</span>
                    <Show when={log.node_id}>
                      <Badge variant="secondary">{nodeName(log.node_id ?? "")}</Badge>
                    </Show>
                    <Show when={log.session_id}>
                      <a
                        class="ml-auto text-[13px] font-medium hover:underline"
                        href={`/sessions/${log.session_id}`}
                      >
                        {sessionTitle(log.session_id ?? "")}
                      </a>
                    </Show>
                  </div>
                  <Show when={log.client_user_agent}>
                    <p class="text-muted-foreground truncate text-[11px]">
                      {log.client_user_agent}
                    </p>
                  </Show>
                  <Show when={log.details !== null && log.details !== undefined}>
                    <details>
                      <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[13px] select-none">
                        {m.settings_audit_details()}
                      </summary>
                      <pre class="bg-muted/40 mt-1 max-h-40 overflow-auto rounded-md p-2 font-mono text-[13px] whitespace-pre-wrap">
                        {JSON.stringify(log.details, null, 2)}
                      </pre>
                    </details>
                  </Show>
                </Card>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
