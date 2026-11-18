import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { ApiError } from "#lib/api/errors.ts";
import { errorCodeMessage, errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { formatRelativeTime } from "#lib/format.ts";
import type {
  ExternalSessionAgentInfo,
  ExternalSessionListResponse,
  ExternalSessionSummary,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { randomUuid } from "#lib/uuid.ts";
import IconDownload from "~icons/lucide/download";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import { toast } from "#lib/components/ui/sonner/index.ts";

export interface ExternalSessionsDialogProps {
  open: boolean;
  /** Counterpart of the Svelte `bind:open`. */
  onOpenChange?: (open: boolean) => void;
}

/** Import a session that was created outside FlexAgent (agent-native history). */
export default function ExternalSessionsDialog(props: ExternalSessionsDialogProps) {
  const navigate = useNavigate();
  const [loading, setLoading] = createSignal(false);
  const [importing, setImporting] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [response, setResponse] = createSignal<ExternalSessionListResponse | null>(null);
  const [selectedNodeId, setSelectedNodeId] = createSignal("");
  let loadGeneration = 0;

  const isHub = () => sync.connection.role === "hub";
  const onlineNodes = () => sync.nodes.filter((node) => node.is_online);
  const targetNodeId = () =>
    isHub() ? selectedNodeId() : (sync.connection.systemInfo?.node_id ?? "");

  const agentInfo = createMemo(
    () => new Map((response()?.agents ?? []).map((agent) => [agent.agent_id, agent])),
  );
  const groupedSessions = createMemo(() => {
    const groups = new Map<string, ExternalSessionSummary[]>();
    for (const session of response()?.sessions ?? []) {
      const list = groups.get(session.agent_id);
      if (list) {
        list.push(session);
      } else {
        groups.set(session.agent_id, [session]);
      }
    }
    return groups;
  });
  const supported = () => (response()?.agents ?? []).some((agent) => agent.supports_list);

  function pickNode(): string {
    if (!isHub()) return "";
    const current = sync.connection.systemInfo?.node_id ?? "";
    if (onlineNodes().some((node) => node.node_id === current)) return current;
    return onlineNodes()[0]?.node_id ?? "";
  }

  async function load(force: boolean): Promise<void> {
    if (isHub() && selectedNodeId() === "") {
      setSelectedNodeId(pickNode());
      if (selectedNodeId() === "") {
        setError(m.error_node_offline());
        return;
      }
    }
    const token = ++loadGeneration;
    setLoading(true);
    setError(null);
    try {
      const next = await sync.connection.client.listExternalSessions({
        nodeId: isHub() ? selectedNodeId() : undefined,
        force,
      });
      if (token !== loadGeneration) return;
      setResponse(next);
    } catch (err) {
      if (token !== loadGeneration) return;
      setError(errorMessage(err));
    } finally {
      if (token === loadGeneration) setLoading(false);
    }
  }

  function handleNodeChange(): void {
    setResponse(null);
    void load(true);
  }

  // Load when the sidebar opens the dialog (the trigger lives outside Dialog.Root,
  // so `onOpenChange` does not fire for it).
  let lastOpen = false;
  createEffect(
    () => props.open,
    (open) => {
      if (open === lastOpen) return;
      lastOpen = open;
      if (!open) return;
      setSelectedNodeId(pickNode());
      void load(true);
    },
  );

  function agentError(agent: ExternalSessionAgentInfo | undefined): string | null {
    if (!agent?.error) return null;
    return errorCodeMessage(agent.error.code, agent.error.params) ?? agent.error.message;
  }

  function updatedLabel(session: ExternalSessionSummary): string | null {
    if (!session.updated_at) return null;
    const parsed = Date.parse(session.updated_at);
    if (Number.isNaN(parsed)) return null;
    return m.external_sessions_updated({ time: formatRelativeTime(parsed) });
  }

  async function importSession(
    session: ExternalSessionSummary,
    options: { mirrorHistory?: boolean } = {},
  ): Promise<void> {
    const agent = agentInfo().get(session.agent_id);
    const mirrorHistory = options.mirrorHistory ?? agent?.supports_history ?? false;
    const key = `${session.agent_id}:${session.agent_session_id}`;
    setImporting(key);
    try {
      const result = await sync.connection.client.importSession({
        command_id: randomUuid(),
        node_id: isHub() ? targetNodeId() : null,
        agent_id: session.agent_id,
        agent_session_id: session.agent_session_id,
        cwd: session.cwd,
        title: session.title,
        mirror_history: mirrorHistory,
      });
      toast.success(m.external_sessions_imported_toast());
      props.onOpenChange?.(false);
      await Promise.all([sync.refreshSessions(), sync.refreshProjects()]);
      navigate(`/sessions/${result.session_id}`);
    } catch (err) {
      // The agent accepted `session/load` but replayed no history. Offer the
      // explicit no-history import instead of creating an empty session silently.
      if (mirrorHistory && err instanceof ApiError && err.code === "HISTORY_UNAVAILABLE") {
        // `toast` in this app has no action button, so describe the next step.
        toast.error(errorMessage(err), {
          description: m.external_sessions_import_no_history(),
          duration: 8000,
        });
      } else {
        toast.error(errorMessage(err));
      }
    } finally {
      setImporting(null);
    }
  }

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }

  return (
    <Dialog.Root open={props.open} onOpenChange={(open) => props.onOpenChange?.(open)}>
      <Dialog.Content class="flex max-h-[85vh] flex-col gap-3 p-5 sm:max-w-2xl">
        <Dialog.Header>
          <Dialog.Title>{m.external_sessions_title()}</Dialog.Title>
          <Dialog.Description>{m.external_sessions_description()}</Dialog.Description>
        </Dialog.Header>

        <div class="flex flex-wrap items-center gap-2">
          <Show when={isHub() && onlineNodes().length > 0}>
            <label class="text-muted-foreground flex items-center gap-2 text-xs">
              <span>{m.external_sessions_node()}</span>
              <select
                value={selectedNodeId()}
                onChange={(event) => {
                  setSelectedNodeId(event.currentTarget.value);
                  handleNodeChange();
                }}
                class="bg-chip border-border h-8 rounded-md border px-2 text-xs"
              >
                <For each={onlineNodes()}>
                  {(node) => <option value={node.node_id}>{node.name}</option>}
                </For>
              </select>
            </label>
          </Show>
          <Button
            variant="outline"
            size="sm"
            class="ml-auto"
            disabled={loading()}
            onClick={() => void load(true)}
          >
            <Show when={loading()} fallback={<IconRefreshCw class="size-4" />}>
              <IconLoaderCircle class="size-4 animate-spin" />
            </Show>
            {m.external_sessions_sync()}
          </Button>
        </div>

        <Show when={error()}>
          {(message) => <p class="text-destructive text-xs">{message()}</p>}
        </Show>

        <div class="min-h-32 overflow-y-auto pr-1">
          <Show
            when={loading() && response() === null}
            fallback={
              <Show when={response()}>
                {(data) => (
                  <>
                    <Show when={data().sessions.length === 0}>
                      <p class="text-muted-foreground py-8 text-center text-sm">
                        {supported()
                          ? m.external_sessions_empty()
                          : m.external_sessions_unsupported()}
                      </p>
                    </Show>

                    <div class="flex flex-col gap-4">
                      <For each={data().agents}>
                        {(agent) => (
                          <section class="flex flex-col gap-2">
                            <header class="flex flex-wrap items-center gap-2">
                              <span class="text-sm font-medium">{agent.name}</span>
                              <Badge variant="outline" class="font-mono text-[10px]">
                                {agent.agent_id}
                              </Badge>
                              <Show when={!agent.supports_list}>
                                <Badge variant="secondary" class="text-[10px]">
                                  {m.external_sessions_agent_unsupported()}
                                </Badge>
                              </Show>
                              <Show when={!agent.supports_history && agent.supports_list}>
                                <Badge variant="secondary" class="text-[10px]">
                                  {m.external_sessions_history_unavailable()}
                                </Badge>
                              </Show>
                            </header>

                            <Show when={agentError(agent)}>
                              {(failure) => <p class="text-destructive text-xs">{failure()}</p>}
                            </Show>

                            <For each={groupedSessions().get(agent.agent_id) ?? []}>
                              {(session) => {
                                const key = `${session.agent_id}:${session.agent_session_id}`;
                                return (
                                  <div class="border-border flex flex-col gap-1 rounded-md border px-3 py-2 sm:flex-row sm:items-center sm:gap-3">
                                    <div class="min-w-0 flex-1">
                                      <p class="truncate text-sm font-medium">
                                        {session.title ?? m.external_sessions_untitled()}
                                      </p>
                                      <p class="text-muted-foreground truncate font-mono text-[11px]">
                                        {pathBasename(session.cwd)} · {session.cwd}
                                      </p>
                                      <Show when={updatedLabel(session)}>
                                        {(label) => (
                                          <p class="text-muted-foreground text-[11px]">{label()}</p>
                                        )}
                                      </Show>
                                    </div>
                                    <div class="flex shrink-0 items-center gap-2">
                                      <Show
                                        when={session.imported_session_id}
                                        fallback={
                                          <Button
                                            size="sm"
                                            variant={agent.supports_history ? "default" : "outline"}
                                            disabled={importing() !== null}
                                            onClick={() => void importSession(session)}
                                            title={
                                              agent.supports_history
                                                ? undefined
                                                : m.external_sessions_history_unavailable_hint()
                                            }
                                          >
                                            <Show
                                              when={importing() === key}
                                              fallback={<IconDownload class="size-4" />}
                                            >
                                              <IconLoaderCircle class="size-4 animate-spin" />
                                            </Show>
                                            {agent.supports_history
                                              ? m.external_sessions_import()
                                              : m.external_sessions_import_no_history()}
                                          </Button>
                                        }
                                      >
                                        {(imported) => (
                                          <a
                                            href={`/sessions/${imported()}`}
                                            class="text-signal text-xs font-medium hover:underline"
                                            onClick={() => props.onOpenChange?.(false)}
                                          >
                                            {m.external_sessions_open()}
                                          </a>
                                        )}
                                      </Show>
                                    </div>
                                  </div>
                                );
                              }}
                            </For>
                          </section>
                        )}
                      </For>
                    </div>
                  </>
                )}
              </Show>
            }
          >
            <div class="text-muted-foreground flex items-center justify-center gap-2 py-10 text-sm">
              <IconLoaderCircle class="size-4 animate-spin" />
              {m.external_sessions_loading()}
            </div>
          </Show>
        </div>
      </Dialog.Content>
    </Dialog.Root>
  );
}
