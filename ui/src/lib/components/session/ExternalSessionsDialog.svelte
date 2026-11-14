<script lang="ts">
  import { goto } from "$app/navigation";
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
  import { sync } from "#lib/stores/app.svelte.ts";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import { toast } from "svelte-sonner";

  let { open = $bindable(false) }: { open: boolean } = $props();

  let loading = $state(false);
  let importing = $state<string | null>(null);
  let error = $state<string | null>(null);
  let response = $state<ExternalSessionListResponse | null>(null);
  let selectedNodeId = $state("");

  const isHub = $derived(sync.connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const targetNodeId = $derived(
    isHub ? selectedNodeId : (sync.connection.systemInfo?.node_id ?? ""),
  );

  const agentInfo = $derived(
    new Map((response?.agents ?? []).map((agent) => [agent.agent_id, agent])),
  );
  const groupedSessions = $derived.by(() => {
    const groups = new Map<string, ExternalSessionSummary[]>();
    for (const session of response?.sessions ?? []) {
      const list = groups.get(session.agent_id);
      if (list) {
        list.push(session);
      } else {
        groups.set(session.agent_id, [session]);
      }
    }
    return groups;
  });
  const supported = $derived((response?.agents ?? []).some((agent) => agent.supports_list));

  function pickNode(): string {
    if (!isHub) return "";
    const current = sync.connection.systemInfo?.node_id ?? "";
    if (onlineNodes.some((node) => node.node_id === current)) return current;
    return onlineNodes[0]?.node_id ?? "";
  }

  async function load(force: boolean) {
    if (isHub && selectedNodeId === "") {
      selectedNodeId = pickNode();
      if (selectedNodeId === "") {
        error = m.error_node_offline();
        return;
      }
    }
    loading = true;
    error = null;
    try {
      response = await sync.connection.client.listExternalSessions({
        nodeId: isHub ? selectedNodeId : undefined,
        force,
      });
    } catch (err) {
      error = errorMessage(err);
    } finally {
      loading = false;
    }
  }

  function handleNodeChange() {
    response = null;
    void load(true);
  }

  // Load when the sidebar opens the dialog (the trigger lives outside Dialog.Root,
  // so `onOpenChange` does not fire for it).
  let lastOpen = false;
  $effect(() => {
    if (open === lastOpen) return;
    lastOpen = open;
    if (!open) return;
    selectedNodeId = pickNode();
    void load(true);
  });

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
  ) {
    const agent = agentInfo.get(session.agent_id);
    const mirrorHistory = options.mirrorHistory ?? agent?.supports_history ?? false;
    const key = `${session.agent_id}:${session.agent_session_id}`;
    importing = key;
    try {
      const result = await sync.connection.client.importSession({
        command_id: crypto.randomUUID(),
        node_id: isHub ? targetNodeId : null,
        agent_id: session.agent_id,
        agent_session_id: session.agent_session_id,
        cwd: session.cwd,
        title: session.title,
        mirror_history: mirrorHistory,
      });
      toast.success(m.external_sessions_imported_toast());
      open = false;
      await Promise.all([sync.refreshSessions(), sync.refreshProjects()]);
      await goto(`/sessions/${result.session_id}`);
    } catch (err) {
      // The agent accepted `session/load` but replayed no history. Offer the
      // explicit no-history import instead of creating an empty session silently.
      if (mirrorHistory && err instanceof ApiError && err.code === "HISTORY_UNAVAILABLE") {
        toast.error(errorMessage(err), {
          action: {
            label: m.external_sessions_import_no_history(),
            onClick: () => void importSession(session, { mirrorHistory: false }),
          },
        });
      } else {
        toast.error(errorMessage(err));
      }
    } finally {
      importing = null;
    }
  }

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85vh] flex-col gap-3 p-5 sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>{m.external_sessions_title()}</Dialog.Title>
      <Dialog.Description>{m.external_sessions_description()}</Dialog.Description>
    </Dialog.Header>

    <div class="flex flex-wrap items-center gap-2">
      {#if isHub && onlineNodes.length > 0}
        <label class="text-muted-foreground flex items-center gap-2 text-xs">
          <span>{m.external_sessions_node()}</span>
          <select
            bind:value={selectedNodeId}
            onchange={handleNodeChange}
            class="bg-chip h-8 rounded-md border border-border px-2 text-xs"
          >
            {#each onlineNodes as node (node.node_id)}
              <option value={node.node_id}>{node.name}</option>
            {/each}
          </select>
        </label>
      {/if}
      <Button
        variant="outline"
        size="sm"
        class="ml-auto"
        disabled={loading}
        onclick={() => void load(true)}
      >
        {#if loading}
          <LoaderCircleIcon class="size-4 animate-spin" />
        {:else}
          <RefreshCwIcon class="size-4" />
        {/if}
        {m.external_sessions_sync()}
      </Button>
    </div>

    {#if error}
      <p class="text-destructive text-xs">{error}</p>
    {/if}

    <div class="min-h-32 overflow-y-auto pr-1">
      {#if loading && response === null}
        <div class="text-muted-foreground flex items-center justify-center gap-2 py-10 text-sm">
          <LoaderCircleIcon class="size-4 animate-spin" />
          {m.external_sessions_loading()}
        </div>
      {:else if response !== null}
        {#if response.sessions.length === 0}
          <p class="text-muted-foreground py-8 text-center text-sm">
            {supported ? m.external_sessions_empty() : m.external_sessions_unsupported()}
          </p>
        {/if}

        <div class="flex flex-col gap-4">
          {#each response.agents as agent (agent.agent_id)}
            {@const sessions = groupedSessions.get(agent.agent_id) ?? []}
            {@const failure = agentError(agent)}
            <section class="flex flex-col gap-2">
              <header class="flex flex-wrap items-center gap-2">
                <span class="text-sm font-medium">{agent.name}</span>
                <Badge variant="outline" class="font-mono text-[10px]">{agent.agent_id}</Badge>
                {#if !agent.supports_list}
                  <Badge variant="secondary" class="text-[10px]">
                    {m.external_sessions_agent_unsupported()}
                  </Badge>
                {/if}
                {#if !agent.supports_history && agent.supports_list}
                  <Badge variant="secondary" class="text-[10px]">
                    {m.external_sessions_history_unavailable()}
                  </Badge>
                {/if}
              </header>

              {#if failure}
                <p class="text-destructive text-xs">{failure}</p>
              {/if}

              {#each sessions as session (session.agent_session_id)}
                {@const key = `${session.agent_id}:${session.agent_session_id}`}
                <div
                  class="flex flex-col gap-1 rounded-md border border-border px-3 py-2 sm:flex-row sm:items-center sm:gap-3"
                >
                  <div class="min-w-0 flex-1">
                    <p class="truncate text-sm font-medium">
                      {session.title ?? m.external_sessions_untitled()}
                    </p>
                    <p class="text-muted-foreground truncate font-mono text-[11px]">
                      {pathBasename(session.cwd)} · {session.cwd}
                    </p>
                    {#if updatedLabel(session)}
                      <p class="text-muted-foreground text-[11px]">{updatedLabel(session)}</p>
                    {/if}
                  </div>
                  <div class="flex shrink-0 items-center gap-2">
                    {#if session.imported_session_id}
                      <a
                        href={`/sessions/${session.imported_session_id}`}
                        class="text-signal text-xs font-medium hover:underline"
                        onclick={() => (open = false)}
                      >
                        {m.external_sessions_open()}
                      </a>
                    {:else}
                      <Button
                        size="sm"
                        variant={agent.supports_history ? "default" : "outline"}
                        disabled={importing !== null}
                        onclick={() => void importSession(session)}
                        title={agent.supports_history
                          ? undefined
                          : m.external_sessions_history_unavailable_hint()}
                      >
                        {#if importing === key}
                          <LoaderCircleIcon class="size-4 animate-spin" />
                        {:else}
                          <DownloadIcon class="size-4" />
                        {/if}
                        {agent.supports_history
                          ? m.external_sessions_import()
                          : m.external_sessions_import_no_history()}
                      </Button>
                    {/if}
                  </div>
                </div>
              {/each}
            </section>
          {/each}
        </div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
