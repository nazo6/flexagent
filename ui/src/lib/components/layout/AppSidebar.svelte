<script lang="ts">
  import { page } from "$app/state";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import * as Tooltip from "#lib/components/ui/tooltip/index.ts";
  import { formatRelativeTimeCompact } from "#lib/format.ts";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import {
    isWaitingSessionStatus,
    sessionAttentionBadgeLabel,
    sessionNeedsAttention,
    sessionStatusLabel,
  } from "#lib/session-status.ts";
  import { pinned } from "#lib/stores/pinned.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import BrandIcon from "#lib/components/BrandIcon.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.svelte";
  import ExternalSessionsDialog from "#lib/components/session/ExternalSessionsDialog.svelte";
  import type { SessionSummary } from "#lib/generated/bindings.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import PanelLeftCloseIcon from "@lucide/svelte/icons/panel-left-close";
  import AlertCircleIcon from "@lucide/svelte/icons/alert-circle";
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import HistoryIcon from "@lucide/svelte/icons/history";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PinIcon from "@lucide/svelte/icons/pin";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import SearchIcon from "@lucide/svelte/icons/search";
  import SettingsIcon from "@lucide/svelte/icons/settings";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";

  interface Props {
    onNavigate?: () => void;
  }

  let { onNavigate }: Props = $props();

  let searchQuery = $state("");
  let collapsedProjects = $state<Record<string, boolean>>({});
  let showAttentionSection = $state(true);
  let showArchivedSection = $state(false);
  let externalOpen = $state(false);

  const currentPath = $derived(page.url.pathname);
  const activeSessionId = $derived(
    currentPath.startsWith("/sessions/") ? currentPath.replace("/sessions/", "") : null,
  );

  function toggleProject(projectId: string) {
    collapsedProjects[projectId] = !collapsedProjects[projectId];
  }

  function isProjectCollapsed(projectId: string): boolean {
    if (searchQuery.trim() !== "") return false;
    return collapsedProjects[projectId] ?? false;
  }

  function matchesSearch(session: SessionSummary, query: string): boolean {
    if (!query) return true;
    const q = query.toLowerCase();
    return (
      session.title.toLowerCase().includes(q) ||
      session.project_id.toLowerCase().includes(q) ||
      session.local_path.toLowerCase().includes(q) ||
      (session.git_branch?.toLowerCase().includes(q) ?? false) ||
      session.agent_id.toLowerCase().includes(q)
    );
  }

  const filteredSessions = $derived(
    sync.sessions
      .filter((session) => session.archived_at === null)
      .filter((session) => matchesSearch(session, searchQuery.trim())),
  );

  // Archived sessions (collapsed by default)
  const archivedSessions = $derived(
    sync.sessions
      .filter((session) => session.archived_at !== null)
      .filter((session) => matchesSearch(session, searchQuery.trim()))
      .toSorted((a, b) => (b.archived_at ?? 0) - (a.archived_at ?? 0)),
  );

  // Sessions needing attention across all projects (waiting sessions stay visible even when archived)
  const needsAttentionSessions = $derived(
    sync.sessions
      .filter((session) => sessionNeedsAttention(session))
      .toSorted((a, b) => b.updated_at - a.updated_at),
  );

  interface ProjectSessionGroup {
    projectId: string;
    projectName: string;
    sessions: SessionSummary[];
    pendingCount: number;
    runningCount: number;
  }

  // Session groups by project
  const projectGroups = $derived.by(() => {
    const groups: ProjectSessionGroup[] = [];
    const projectMap = new Map<string, string>();

    for (const p of sync.projects) {
      projectMap.set(p.project_id, p.name);
    }

    const allProjectIds = new Set<string>();
    for (const s of filteredSessions) {
      allProjectIds.add(s.project_id);
    }
    for (const p of sync.projects) {
      allProjectIds.add(p.project_id);
    }

    for (const pId of allProjectIds) {
      const projSessions = filteredSessions
        .filter((s) => s.project_id === pId)
        .toSorted((a, b) => {
          const aPinned = pinned.isPinned(a.session_id) ? 1 : 0;
          const bPinned = pinned.isPinned(b.session_id) ? 1 : 0;
          if (aPinned !== bPinned) return bPinned - aPinned;
          return b.updated_at - a.updated_at;
        });

      if (searchQuery.trim() !== "" && projSessions.length === 0) {
        continue;
      }

      const pendingCount = projSessions.filter((s) => isWaitingSessionStatus(s.status)).length;
      const runningCount = projSessions.filter((s) => s.status === "running").length;

      groups.push({
        projectId: pId,
        projectName: projectMap.get(pId) ?? pId,
        sessions: projSessions,
        pendingCount,
        runningCount,
      });
    }

    // Sort projects with the most recently updated sessions first
    return groups.toSorted((a, b) => {
      const aLatest = a.sessions[0]?.updated_at ?? 0;
      const bLatest = b.sessions[0]?.updated_at ?? 0;
      return bLatest - aLatest;
    });
  });

  function getNode(nodeId: string) {
    return sync.nodes.find((n) => n.node_id === nodeId);
  }

  function handleLinkClick() {
    onNavigate?.();
  }
</script>

<aside class="bg-sidebar flex h-full w-full flex-col border-r border-border">
  <!-- Sidebar header. In a Window Controls Overlay window this is the left
       half of the title bar; see the `.wco-*` rules in app.css. -->
  <div
    class="wco-titlebar wco-sidebar-header flex items-center justify-between border-b border-border px-3.5 py-3"
  >
    <a
      href="/"
      class="flex items-center gap-2.5 font-semibold tracking-tight hover:opacity-80 transition-opacity"
      onclick={handleLinkClick}
    >
      <BrandIcon class="size-7" />
      <span class="text-sm">FlexAgent</span>
    </a>
    <button
      type="button"
      class="hover:bg-accent text-muted-foreground hover:text-foreground hidden size-7 items-center justify-center rounded-md border lg:flex"
      onclick={() => layoutState.toggleSidebar()}
      title={m.shell_hide_sidebar()}
      aria-label={m.shell_hide_sidebar()}
    >
      <PanelLeftCloseIcon class="size-4" />
    </button>
  </div>

  <!-- Quick navigation -->
  <nav class="flex flex-col gap-0.5 border-b border-border p-2 text-[13px]">
    <a
      href="/"
      onclick={handleLinkClick}
      class={cn(
        "flex items-center justify-between rounded-md px-2.5 py-1.5 font-medium transition-colors",
        currentPath === "/"
          ? "bg-signal-soft text-foreground"
          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
      )}
    >
      <div class="flex items-center gap-2">
        <PlusIcon class="size-4" />
        <span>{m.shell_new_session()}</span>
      </div>
    </a>

    <a
      href="/projects"
      onclick={handleLinkClick}
      class={cn(
        "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
        currentPath.startsWith("/projects")
          ? "bg-signal-soft text-foreground"
          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
      )}
    >
      <FolderGit2Icon class="size-4" />
      <span>{m.shell_projects()}</span>
    </a>

    <button
      type="button"
      onclick={() => (externalOpen = true)}
      class="text-muted-foreground hover:bg-accent/50 hover:text-foreground flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left font-medium transition-colors"
    >
      <HistoryIcon class="size-4" />
      <span>{m.shell_external_sessions()}</span>
    </button>

    <a
      href="/search"
      onclick={handleLinkClick}
      class={cn(
        "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
        currentPath.startsWith("/search")
          ? "bg-signal-soft text-foreground"
          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
      )}
    >
      <SearchIcon class="size-4" />
      <span>{m.shell_full_text_search()}</span>
    </a>

    <a
      href="/settings"
      onclick={handleLinkClick}
      class={cn(
        "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
        currentPath.startsWith("/settings")
          ? "bg-signal-soft text-foreground"
          : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
      )}
    >
      <SettingsIcon class="size-4" />
      <span>{m.shell_settings()}</span>
    </a>
  </nav>

  <!-- Search bar -->
  <div class="border-b px-2 py-2">
    <div class="relative flex items-center">
      <SearchIcon class="text-muted-foreground absolute left-2 size-4 pointer-events-none" />
      <input
        type="text"
        bind:value={searchQuery}
        placeholder={m.shell_search_placeholder()}
        aria-label={m.shell_search_placeholder()}
        class="bg-chip placeholder:text-muted-foreground focus-visible:ring-ring/40 h-8 w-full rounded-md border border-border pr-2 pl-7 text-[13px] outline-none focus-visible:ring-2"
      />
    </div>
  </div>

  <!-- Session list (grouped by project) -->
  <div class="scrollbar-thin flex-1 overflow-y-auto p-2">
    <!-- Needs attention section (shown at the top only when there are pending approvals or errors) -->
    {#if needsAttentionSessions.length > 0 && searchQuery.trim() === ""}
      <div class="mb-3 rounded-lg border border-destructive/30 bg-destructive/5 p-1.5">
        <button
          type="button"
          class="text-destructive flex w-full items-center justify-between px-1 py-0.5 text-[11px] font-semibold tracking-wider uppercase select-none"
          onclick={() => (showAttentionSection = !showAttentionSection)}
        >
          <span class="flex items-center gap-1.5">
            <AlertCircleIcon class="size-3" />
            <span>{m.shell_needs_attention()}</span>
            <span class="rounded-full bg-destructive/15 px-1.5 py-0.2 text-[11px] text-destructive">
              {needsAttentionSessions.length}
            </span>
          </span>
          {#if showAttentionSection}
            <ChevronDownIcon class="size-3" />
          {:else}
            <ChevronRightIcon class="size-3" />
          {/if}
        </button>

        {#if showAttentionSection}
          <ul class="flex list-none flex-col gap-0.5 p-0 mt-1">
            {#each needsAttentionSessions as session (session.session_id)}
              {@render sessionRow(session)}
            {/each}
          </ul>
        {/if}
      </div>
    {/if}

    <!-- List by project -->
    {#if projectGroups.length === 0}
      <div class="text-muted-foreground p-3 text-center text-[13px]">
        {searchQuery ? m.shell_no_matching_results() : m.shell_no_projects_yet()}
      </div>
    {:else}
      <div class="flex flex-col gap-2.5">
        {#each projectGroups as group (group.projectId)}
          {@const isCollapsed = isProjectCollapsed(group.projectId)}
          <div class="flex flex-col">
            <!-- Project header -->
            <div
              class="group/proj flex items-center justify-between rounded-md px-1.5 py-1 text-[13px] hover:bg-muted/40 transition-colors"
            >
              <button
                type="button"
                class="flex min-w-0 flex-1 items-center gap-1.5 text-left font-semibold text-foreground/90 select-none"
                onclick={() => toggleProject(group.projectId)}
              >
                <span class="text-muted-foreground shrink-0">
                  {#if isCollapsed}
                    <ChevronRightIcon class="size-4" />
                  {:else}
                    <ChevronDownIcon class="size-4" />
                  {/if}
                </span>
                <FolderGit2Icon class="size-4 text-muted-foreground shrink-0" />
                <span class="truncate">{group.projectName}</span>
                {#if group.pendingCount > 0}
                  <Badge variant="destructive" class="px-1 py-0 text-[11px] leading-tight shrink-0">
                    {group.pendingCount}
                  </Badge>
                {/if}
                {#if group.runningCount > 0}
                  <span
                    class="size-1.5 rounded-full bg-ok shrink-0"
                    title={m.shell_has_running_sessions()}
                  ></span>
                {/if}
              </button>

              <div class="flex items-center gap-1 shrink-0 ml-1">
                <span class="text-muted-foreground text-[11px]">
                  {group.sessions.length}
                </span>
                <a
                  href={`/?project=${encodeURIComponent(group.projectId)}`}
                  onclick={handleLinkClick}
                  class="hover:bg-accent text-muted-foreground/60 hover:text-foreground rounded p-0.5 transition-colors"
                  title={m.shell_start_new_session_in_project({ project: group.projectName })}
                >
                  <PlusIcon class="size-3" />
                </a>
              </div>
            </div>

            <!-- Sessions in the project -->
            {#if !isCollapsed}
              <ul
                class="ml-2.5 border-l border-border/60 pl-2 flex list-none flex-col gap-0.5 p-0 mt-0.5"
              >
                {#each group.sessions as session (session.session_id)}
                  {@render sessionRow(session)}
                {/each}
                {#if group.sessions.length === 0}
                  <li class="py-1 px-1.5 text-[11px] text-muted-foreground">
                    <a
                      href={`/?project=${encodeURIComponent(group.projectId)}`}
                      onclick={handleLinkClick}
                      class="hover:underline flex items-center gap-1 text-primary/80"
                    >
                      <PlusIcon class="size-3" />
                      <span>{m.shell_start_session()}</span>
                    </a>
                  </li>
                {/if}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <!-- Archived sessions (collapsible) -->
    {#if archivedSessions.length > 0}
      <div class="mt-3 border-t pt-2">
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-0.5 text-[11px] font-semibold tracking-wider uppercase select-none"
          onclick={() => (showArchivedSection = !showArchivedSection)}
        >
          <span class="flex items-center gap-1.5">
            <ArchiveIcon class="size-3" />
            <span>{m.shell_archived()}</span>
            <span class="bg-muted rounded-full px-1.5 py-0.2 text-[11px]">
              {archivedSessions.length}
            </span>
          </span>
          {#if showArchivedSection}
            <ChevronDownIcon class="size-3" />
          {:else}
            <ChevronRightIcon class="size-3" />
          {/if}
        </button>

        {#if showArchivedSection}
          <ul class="mt-1 flex list-none flex-col gap-0.5 p-0">
            {#each archivedSessions as session (session.session_id)}
              {@render sessionRow(session)}
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </div>

  <!-- Footer: problems and the connection target selector -->
  <div class="border-border flex flex-col gap-2 border-t p-2">
    <ProblemBadge label class="w-full justify-start" />
    <ConnectionMenu variant="scope" />
  </div>
</aside>

<ExternalSessionsDialog bind:open={externalOpen} />

{#snippet sessionRow(session: SessionSummary)}
  {@const isActive = activeSessionId === session.session_id}
  {@const isArchived = session.archived_at !== null}
  {@const isWaiting = isWaitingSessionStatus(session.status)}
  {@const isAttention = sessionNeedsAttention(session)}
  {@const attentionLabel = isAttention ? sessionAttentionBadgeLabel(session.status) : null}
  {@const node = getNode(session.node_id)}
  {@const nodeAvail = getNodeAvailability(node, sync.wsConnected)}
  {@const isPinned = pinned.isPinned(session.session_id)}
  <li>
    <Tooltip.Provider delayDuration={400}>
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <a
              {...props}
              href={`/sessions/${session.session_id}`}
              onclick={handleLinkClick}
              class={cn(
                "group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-[13px] transition-colors",
                isActive
                  ? "bg-signal-soft text-foreground font-medium"
                  : isWaiting
                    ? "text-foreground font-semibold hover:bg-warn-soft/50"
                    : isAttention
                      ? "text-foreground font-semibold hover:bg-danger-soft/50"
                      : "text-foreground/80 hover:bg-accent/40 hover:text-foreground",
                isArchived && !isWaiting && "opacity-70",
              )}
            >
              <!-- Node status dot -->
              <span class={cn("size-1.5 shrink-0 rounded-full", nodeAvail.dotColorClass)}></span>

              <!-- The agent is answering / working right now -->
              {#if session.status === "running"}
                <span
                  class="text-signal shrink-0"
                  role="img"
                  title={m.status_running()}
                  aria-label={m.status_running()}
                >
                  <LoaderCircleIcon class="size-3.5 animate-spin" aria-hidden="true" />
                </span>
              {/if}

              {#if isAttention}
                {#if isWaiting}
                  <TriangleAlertIcon class="text-warn size-3.5 shrink-0" aria-hidden="true" />
                {:else}
                  <AlertCircleIcon class="text-danger size-3.5 shrink-0" aria-hidden="true" />
                {/if}
              {/if}

              <!-- Title -->
              <span class="min-w-0 flex-1 truncate">{session.title}</span>

              {#if attentionLabel}
                <span
                  class={cn(
                    "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold leading-none",
                    isWaiting ? "bg-warn/15 text-warn" : "bg-danger/15 text-danger",
                  )}
                >
                  {attentionLabel}
                </span>
              {/if}

              <div class="flex shrink-0 items-center text-muted-foreground text-[11px]">
                <!-- Resting metadata. Hidden while the actions are open so the
                     two clusters never take horizontal space at the same time. -->
                <span
                  class="flex items-center gap-1.5 group-hover:hidden group-focus-within:hidden"
                >
                  {#if isPinned}
                    <PinIcon
                      class="text-signal size-3 shrink-0"
                      fill="currentColor"
                      aria-hidden="true"
                    />
                  {/if}
                  <span class="tabular-nums">
                    {formatRelativeTimeCompact(session.updated_at)}
                  </span>
                </span>

                <!-- Actions. Zero-width until hover/focus: they stay focusable
                     from the keyboard but do not truncate the title at rest. -->
                <div
                  class="flex w-0 shrink-0 items-center gap-1 overflow-hidden opacity-0 transition-opacity group-hover:ml-1.5 group-hover:w-auto group-hover:opacity-100 group-focus-within:ml-1.5 group-focus-within:w-auto group-focus-within:opacity-100"
                >
                  <!-- Archive / delete menu -->
                  <SessionActionsMenu {session} align="end" iconClass="size-3.5" />

                  <!-- Pin button -->
                  <button
                    type="button"
                    class={cn(
                      "hover:text-foreground p-0.5 transition-colors",
                      isPinned && "text-signal",
                    )}
                    title={isPinned ? m.shell_unpin() : m.shell_pin()}
                    onclick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      pinned.toggle(session.session_id);
                    }}
                  >
                    <PinIcon class="size-3" fill={isPinned ? "currentColor" : "none"} />
                  </button>
                </div>
              </div>
            </a>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content
          side="right"
          align="start"
          class="max-w-[300px] flex-col items-stretch gap-1.5 px-2.5 py-2 text-left leading-snug"
        >
          <span class="font-medium break-words">
            {`${session.title}${isArchived ? ` (${m.shell_archived()})` : ""}`}
          </span>
          <span class="text-background/70 flex items-center gap-1.5">
            <FolderGit2Icon class="size-3 shrink-0" />
            <span class="truncate">{session.project_id}</span>
          </span>
          {#if session.git_branch}
            <span class="text-background/70 flex items-center gap-1.5">
              <GitBranchIcon class="size-3 shrink-0" />
              <span class="truncate font-mono">{session.git_branch}</span>
            </span>
          {/if}
          <span class="text-background/70 flex items-start gap-1.5">
            <FolderIcon class="mt-0.5 size-3 shrink-0" />
            <span class="font-mono text-[11px] break-all">{session.local_path}</span>
          </span>
          <span class="text-background/70 flex items-center gap-1.5">
            <span class={cn("size-1.5 shrink-0 rounded-full", nodeAvail.dotColorClass)}></span>
            <span class="truncate">
              {sessionStatusLabel(session.status)}
              {" · "}
              {m.shell_node_status({
                node: node?.name ?? session.node_id,
                status: nodeAvail.statusText,
              })}
            </span>
          </span>
        </Tooltip.Content>
      </Tooltip.Root>
    </Tooltip.Provider>
  </li>
{/snippet}
