<script lang="ts">
  import { goto } from "$app/navigation";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import SessionStatusBadge from "#lib/components/SessionStatusBadge.svelte";
  import { ApiError } from "#lib/api/errors.ts";
  import { errorCodeMessage, errorMessage } from "#lib/api/error-messages.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type {
    ExternalSessionAgentInfo,
    ExternalSessionListResponse,
    ExternalSessionSummary,
    ProjectSummary,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { connection, sync } from "#lib/stores/app.svelte.ts";
  import { randomUuid } from "#lib/uuid.ts";
  import { toast } from "svelte-sonner";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import HistoryIcon from "@lucide/svelte/icons/history";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";

  type Tab = "unimported" | "imported" | "all";

  /** Refresh a cached scan in the background once it is older than this. */
  const CACHE_REFRESH_AFTER_MS = 30_000;

  let response = $state<ExternalSessionListResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selectedNodeId = $state("");
  let lastSyncedAt = $state<number | null>(null);
  let tab = $state<Tab>("unimported");
  let query = $state("");
  let agentFilter = $state("");
  let busyKey = $state<string | null>(null);
  let pendingRefresh = $state<ExternalSessionSummary | null>(null);
  let refreshOpen = $state(false);
  let loadGeneration = 0;
  let autoLoaded = false;

  const isHub = $derived(connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const targetNodeId = $derived(isHub ? selectedNodeId : (connection.systemInfo?.node_id ?? ""));
  const targetNodeOs = $derived(sync.nodes.find((node) => node.node_id === targetNodeId)?.os ?? "");
  const caseInsensitivePaths = $derived(targetNodeOs === "windows");

  const agentInfo = $derived(
    new Map((response?.agents ?? []).map((agent) => [agent.agent_id, agent])),
  );
  const supportedAgents = $derived((response?.agents ?? []).filter((agent) => agent.supports_list));
  const issueAgents = $derived(
    (response?.agents ?? []).filter((agent) => !agent.supports_list || agent.error !== null),
  );

  const sessions = $derived(response?.sessions ?? []);
  const counts = $derived.by(() => {
    let imported = 0;
    for (const session of sessions) {
      if (session.imported_session_id) imported += 1;
    }
    return {
      all: sessions.length,
      imported,
      unimported: sessions.length - imported,
    };
  });

  interface ProjectPath {
    projectId: string;
    name: string;
    path: string;
  }

  const projectPaths = $derived.by(() => {
    const paths: ProjectPath[] = [];
    for (const project of sync.projects) {
      addProjectPaths(paths, project, targetNodeId);
    }
    return paths;
  });

  function addProjectPaths(paths: ProjectPath[], project: ProjectSummary, nodeId: string): void {
    for (const binding of project.bindings) {
      if (binding.node_id !== nodeId) continue;
      paths.push({
        projectId: project.project_id,
        name: project.name,
        path: normalizePath(binding.local_path),
      });
    }
  }

  function normalizePath(path: string): string {
    const normalized = path.replaceAll("\\", "/").replace(/\/+$/, "");
    return caseInsensitivePaths ? normalized.toLowerCase() : normalized;
  }

  function projectFor(session: ExternalSessionSummary): ProjectPath | null {
    const cwd = normalizePath(session.cwd);
    let best: ProjectPath | null = null;
    for (const candidate of projectPaths) {
      const matches = cwd === candidate.path || cwd.startsWith(`${candidate.path}/`);
      if (matches && (best === null || candidate.path.length > best.path.length)) {
        best = candidate;
      }
    }
    return best;
  }

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }

  function parseTimestamp(value: string | null | undefined): number | null {
    if (!value) return null;
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
  }

  function sortTimestamp(session: ExternalSessionSummary): number {
    return parseTimestamp(session.updated_at) ?? session.imported_updated_at ?? 0;
  }

  function updatedLabel(session: ExternalSessionSummary): string | null {
    const parsed = parseTimestamp(session.updated_at);
    return parsed === null
      ? null
      : m.external_sessions_updated({ time: formatRelativeTime(parsed) });
  }

  function agentName(agentId: string): string {
    return agentInfo.get(agentId)?.name ?? agentId;
  }

  function supportsHistory(agentId: string): boolean {
    return agentInfo.get(agentId)?.supports_history ?? false;
  }

  function agentError(agent: ExternalSessionAgentInfo | undefined): string | null {
    if (!agent?.error) return null;
    return errorCodeMessage(agent.error.code, agent.error.params) ?? agent.error.message;
  }

  const filteredSessions = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    return sessions
      .filter((session) => {
        if (tab === "unimported" && session.imported_session_id) return false;
        if (tab === "imported" && !session.imported_session_id) return false;
        if (agentFilter !== "" && session.agent_id !== agentFilter) return false;
        if (needle === "") return true;
        const haystack = [
          session.title,
          session.imported_title,
          session.cwd,
          session.agent_id,
          agentName(session.agent_id),
        ];
        return haystack.some((value) => value?.toLowerCase().includes(needle));
      })
      .toSorted((a, b) => sortTimestamp(b) - sortTimestamp(a));
  });

  interface SessionGroup {
    key: string;
    name: string;
    sessions: ExternalSessionSummary[];
  }

  const groupedSessions = $derived.by(() => {
    const groups = new Map<string, SessionGroup>();
    for (const session of filteredSessions) {
      const project = projectFor(session);
      const folder = pathBasename(session.cwd) || session.cwd;
      const key = project?.projectId ?? `path:${folder}`;
      const name = project?.name ?? folder;
      const group = groups.get(key);
      if (group) {
        group.sessions.push(session);
      } else {
        groups.set(key, { key, name, sessions: [session] });
      }
    }
    return [...groups.values()].toSorted((a, b) => a.name.localeCompare(b.name));
  });

  function pickNode(): string {
    if (!isHub) return "";
    const current = connection.systemInfo?.node_id ?? "";
    if (onlineNodes.some((node) => node.node_id === current)) return current;
    return onlineNodes[0]?.node_id ?? "";
  }

  async function load(force: boolean) {
    if (isHub && selectedNodeId === "") {
      error = m.error_node_offline();
      return;
    }
    const token = ++loadGeneration;
    loading = true;
    error = null;
    try {
      const next = await sync.connection.client.listExternalSessions({
        nodeId: isHub ? selectedNodeId : undefined,
        force,
      });
      if (token !== loadGeneration) return;
      response = next;
      const scannedAt = next.scanned_at ?? 0;
      lastSyncedAt = scannedAt > 0 ? scannedAt : Date.now();
      if (!force && Date.now() - lastSyncedAt > CACHE_REFRESH_AFTER_MS) {
        // Show the cached scan immediately, then refresh it without blocking.
        void load(true);
      }
    } catch (err) {
      if (token !== loadGeneration) return;
      error = errorMessage(err);
    } finally {
      if (token === loadGeneration) loading = false;
    }
  }

  // Load once the connection (and, on a hub, the node list) is available.
  $effect(() => {
    if (autoLoaded) return;
    const node = pickNode();
    if (isHub ? node === "" : connection.systemInfo === null) return;
    autoLoaded = true;
    selectedNodeId = isHub ? node : "";
    void load(false);
  });

  function handleNodeChange() {
    response = null;
    lastSyncedAt = null;
    void load(true);
  }

  async function openSession(
    session: ExternalSessionSummary,
    options: { refresh?: boolean; mirrorHistory?: boolean } = {},
  ) {
    const importedId = session.imported_session_id;
    const refresh = options.refresh ?? false;
    if (importedId && !refresh) {
      await goto(`/sessions/${importedId}`);
      return;
    }
    if (refresh && importedId === null) return;
    if (refresh && !supportsHistory(session.agent_id)) {
      toast.error(m.external_sessions_refresh_unavailable());
      return;
    }

    const key = `${session.agent_id}:${session.agent_session_id}`;
    busyKey = key;
    try {
      const result = await sync.connection.client.importSession({
        command_id: randomUuid(),
        node_id: isHub ? targetNodeId : null,
        agent_id: session.agent_id,
        agent_session_id: session.agent_session_id,
        cwd: session.cwd,
        title: refresh ? (session.imported_title ?? session.title) : session.title,
        mirror_history: refresh
          ? true
          : (options.mirrorHistory ?? supportsHistory(session.agent_id)),
        replace_session_id: refresh ? importedId : null,
      });
      toast.success(
        refresh ? m.external_sessions_refreshed_toast() : m.external_sessions_imported_toast(),
      );
      await Promise.all([sync.refreshSessions(), sync.refreshProjects()]);
      await goto(`/sessions/${result.session_id}`);
    } catch (err) {
      // The agent accepted `session/load` but replayed no history. Offer the
      // explicit no-history import instead of creating an empty session silently.
      if (
        !refresh &&
        options.mirrorHistory !== false &&
        supportsHistory(session.agent_id) &&
        err instanceof ApiError &&
        err.code === "HISTORY_UNAVAILABLE"
      ) {
        toast.error(errorMessage(err), {
          action: {
            label: m.external_sessions_import_no_history(),
            onClick: () => void openSession(session, { mirrorHistory: false }),
          },
        });
      } else {
        toast.error(errorMessage(err));
      }
    } finally {
      busyKey = null;
    }
  }

  function tabButtonClass(active: boolean): string {
    return active
      ? "border-signal text-foreground border-b-2 px-2.5 py-1.5 text-[13px] font-medium"
      : "text-muted-foreground hover:text-foreground border-b-2 border-transparent px-2.5 py-1.5 text-[13px] font-medium";
  }

  function askRefresh(session: ExternalSessionSummary) {
    pendingRefresh = session;
    refreshOpen = true;
  }

  async function confirmRefresh() {
    const session = pendingRefresh;
    pendingRefresh = null;
    refreshOpen = false;
    if (session) await openSession(session, { refresh: true });
  }
</script>

<svelte:head>
  <title>{m.external_sessions_page_title()}</title>
</svelte:head>

<div class="desk-page">
  <div class="flex flex-wrap items-end justify-between gap-3">
    <div class="flex flex-col gap-1">
      <div class="flex items-center gap-2">
        <HistoryIcon class="text-muted-foreground size-5" />
        <h1 class="text-xl font-semibold tracking-tight">{m.external_sessions_title()}</h1>
      </div>
      <p class="text-muted-foreground max-w-2xl text-[13px]">
        {m.external_sessions_description()}
      </p>
    </div>
    <div class="flex items-center gap-2">
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void load(true)}>
        {#if loading}
          <LoaderCircleIcon class="size-4 animate-spin" />
        {:else}
          <RefreshCwIcon class="size-4" />
        {/if}
        {m.external_sessions_sync()}
      </Button>
    </div>
  </div>

  <div class="flex flex-wrap items-center gap-2">
    {#if isHub && onlineNodes.length > 0}
      <label class="text-muted-foreground flex items-center gap-2 text-xs">
        <span>{m.external_sessions_node()}</span>
        <select
          bind:value={selectedNodeId}
          onchange={handleNodeChange}
          class="bg-chip border-border h-8 rounded-md border px-2 text-xs"
        >
          {#each onlineNodes as node (node.node_id)}
            <option value={node.node_id}>{node.name}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if lastSyncedAt !== null}
      <span class="text-muted-foreground text-[11px]">
        {m.external_sessions_last_synced({
          time: formatRelativeTime(lastSyncedAt),
        })}
      </span>
    {/if}
  </div>

  <div class="flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative min-w-56 flex-1">
        <SearchIcon
          class="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4"
        />
        <Input
          bind:value={query}
          class="pl-8"
          placeholder={m.external_sessions_search_placeholder()}
          aria-label={m.external_sessions_search_placeholder()}
        />
      </div>
      <select
        bind:value={agentFilter}
        class="bg-chip border-border h-9 rounded-md border px-2 text-[13px]"
        aria-label={m.external_sessions_all_agents()}
      >
        <option value="">{m.external_sessions_all_agents()}</option>
        {#each supportedAgents as agent (agent.agent_id)}
          <option value={agent.agent_id}>{agent.name}</option>
        {/each}
      </select>
    </div>

    <div class="border-border flex flex-wrap items-center gap-1 border-b">
      <button
        type="button"
        class={tabButtonClass(tab === "unimported")}
        onclick={() => (tab = "unimported")}
      >
        {m.external_sessions_tab_unimported()}
        <span class="text-muted-foreground text-[11px]">{counts.unimported}</span>
      </button>
      <button
        type="button"
        class={tabButtonClass(tab === "imported")}
        onclick={() => (tab = "imported")}
      >
        {m.external_sessions_tab_imported()}
        <span class="text-muted-foreground text-[11px]">{counts.imported}</span>
      </button>
      <button type="button" class={tabButtonClass(tab === "all")} onclick={() => (tab = "all")}>
        {m.external_sessions_tab_all()}
        <span class="text-muted-foreground text-[11px]">{counts.all}</span>
      </button>
    </div>

    {#if error}
      <p class="text-destructive text-xs">{error}</p>
    {/if}

    {#if loading && response === null}
      <div class="text-muted-foreground flex items-center justify-center gap-2 py-16 text-sm">
        <LoaderCircleIcon class="size-4 animate-spin" />
        {m.external_sessions_loading()}
      </div>
    {:else if response === null}
      {#if isHub && onlineNodes.length === 0}
        <p class="text-muted-foreground py-12 text-center text-sm">{m.error_node_offline()}</p>
      {/if}
    {:else if sessions.length === 0}
      <p class="text-muted-foreground py-12 text-center text-sm">
        {m.external_sessions_empty()}
      </p>
    {:else if filteredSessions.length === 0}
      <p class="text-muted-foreground py-12 text-center text-sm">
        {query.trim() !== "" || agentFilter !== ""
          ? m.external_sessions_no_matches()
          : tab === "unimported"
            ? m.external_sessions_no_unimported()
            : tab === "imported"
              ? m.external_sessions_no_imported()
              : m.external_sessions_no_matches()}
      </p>
    {:else}
      <div class="flex flex-col gap-5">
        {#each groupedSessions as group (group.key)}
          <section class="flex flex-col gap-2">
            <header class="flex items-center gap-2 px-0.5">
              <FolderGit2Icon class="text-muted-foreground size-4" />
              <h2 class="text-[13px] font-semibold">{group.name}</h2>
              <span class="text-muted-foreground text-[11px]">{group.sessions.length}</span>
            </header>
            <div class="flex flex-col gap-1.5">
              {#each group.sessions as session (session.agent_id + ":" + session.agent_session_id)}
                {@const importedId = session.imported_session_id}
                {@const key = session.agent_id + ":" + session.agent_session_id}
                {@const busy = busyKey === key}
                {@const history = supportsHistory(session.agent_id)}
                <div
                  class="bg-card/60 hover:bg-accent/30 border-border flex flex-wrap items-center gap-3 rounded-lg border px-3 py-2.5 transition-colors"
                >
                  <div class="min-w-0 flex-1">
                    <div class="flex min-w-0 flex-wrap items-center gap-1.5">
                      <p class="min-w-0 truncate text-sm font-medium">
                        {importedId
                          ? (session.imported_title ??
                            session.title ??
                            m.external_sessions_untitled())
                          : (session.title ?? m.external_sessions_untitled())}
                      </p>
                      <Badge variant="outline" class="font-mono text-[10px]">
                        {agentName(session.agent_id)}
                      </Badge>
                      {#if importedId && session.imported_status}
                        <SessionStatusBadge status={session.imported_status} />
                      {/if}
                      {#if session.needs_sync && importedId}
                        <Badge
                          variant="secondary"
                          class="border-warn/30 bg-warn-soft text-warn text-[10px]"
                        >
                          <TriangleAlertIcon class="size-3" />
                          {m.external_sessions_needs_sync()}
                        </Badge>
                      {/if}
                    </div>
                    <p class="text-muted-foreground truncate font-mono text-[11px]">
                      {pathBasename(session.cwd)} · {session.cwd}
                    </p>
                    <div
                      class="text-muted-foreground flex flex-wrap items-center gap-2 text-[11px]"
                    >
                      {#if updatedLabel(session)}
                        <span>{updatedLabel(session)}</span>
                      {/if}
                      {#if importedId && session.title && session.title !== session.imported_title}
                        <span class="truncate">
                          {m.external_sessions_agent_title({ title: session.title })}
                        </span>
                      {/if}
                      {#if !history}
                        <span title={m.external_sessions_history_unavailable_hint()}>
                          {m.external_sessions_history_unavailable()}
                        </span>
                      {/if}
                    </div>
                  </div>
                  <div class="flex shrink-0 flex-wrap items-center gap-1.5">
                    {#if importedId}
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={busy || !history}
                        title={history
                          ? m.external_sessions_refresh()
                          : m.external_sessions_refresh_unavailable()}
                        aria-label={m.external_sessions_refresh()}
                        onclick={() => askRefresh(session)}
                      >
                        {#if busy}
                          <LoaderCircleIcon class="size-4 animate-spin" />
                        {:else}
                          <RefreshCwIcon class="size-4" />
                        {/if}
                        {#if session.needs_sync}
                          {m.external_sessions_refresh()}
                        {/if}
                      </Button>
                    {/if}
                    <Button
                      size="sm"
                      variant={importedId || history ? "default" : "outline"}
                      disabled={busy}
                      title={!history && !importedId
                        ? m.external_sessions_history_unavailable_hint()
                        : undefined}
                      onclick={() => void openSession(session)}
                    >
                      {#if busy}
                        <LoaderCircleIcon class="size-4 animate-spin" />
                      {:else if importedId}
                        <FolderGit2Icon class="size-4" />
                      {:else}
                        <DownloadIcon class="size-4" />
                      {/if}
                      {m.external_sessions_open()}
                    </Button>
                  </div>
                </div>
              {/each}
            </div>
          </section>
        {/each}
      </div>
    {/if}

    {#if issueAgents.length > 0}
      <details class="border-border bg-card/40 rounded-lg border px-3 py-2">
        <summary class="text-muted-foreground cursor-pointer text-xs font-medium select-none">
          {m.external_sessions_detection_issues()} ({issueAgents.length})
        </summary>
        <div class="mt-2 flex flex-col gap-1.5">
          {#each issueAgents as agent (agent.agent_id)}
            <div class="flex flex-wrap items-center gap-2 text-[11px]">
              <span class="font-medium">{agent.name}</span>
              {#if !agent.supports_list}
                <Badge variant="secondary" class="text-[10px]">
                  {m.external_sessions_agent_unsupported()}
                </Badge>
              {/if}
              {#if agentError(agent)}
                <span class="text-destructive">{agentError(agent)}</span>
              {/if}
            </div>
          {/each}
        </div>
      </details>
    {/if}
  </div>
</div>

<AlertDialog.Root bind:open={refreshOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.external_sessions_refresh_confirm_title()}</AlertDialog.Title>
      <AlertDialog.Description>
        {m.external_sessions_refresh_confirm_description()}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action onclick={() => void confirmRefresh()}>
        {m.external_sessions_refresh()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
