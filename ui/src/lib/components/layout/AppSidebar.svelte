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
  import { motion } from "#lib/motion.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { flip } from "svelte/animate";
  import { slide } from "svelte/transition";
  import BrandIcon from "#lib/components/BrandIcon.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.svelte";
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
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    onNavigate?: () => void;
  }

  let { onNavigate }: Props = $props();

  let searchQuery = $state("");
  let attentionOnly = $state(false);
  let showArchivedSection = $state(false);

  /**
   * Projects the user collapsed are remembered per device. The sidebar is the
   * only place that knows about them, so the state stays local instead of
   * travelling to the node or the hub.
   */
  const COLLAPSED_PROJECTS_KEY = "fxg:sidebar_collapsed_projects";

  function loadCollapsedProjects(): Record<string, boolean> {
    if (typeof localStorage === "undefined") return {};
    try {
      const stored: unknown = JSON.parse(localStorage.getItem(COLLAPSED_PROJECTS_KEY) ?? "[]");
      if (!Array.isArray(stored)) return {};
      const collapsed: Record<string, boolean> = {};
      for (const projectId of stored) {
        if (typeof projectId === "string") collapsed[projectId] = true;
      }
      return collapsed;
    } catch {
      // Storage may be disabled or hold an old shape; start expanded.
      return {};
    }
  }

  let collapsedProjects = $state<Record<string, boolean>>(loadCollapsedProjects());

  function saveCollapsedProjects(): void {
    if (typeof localStorage === "undefined") return;
    try {
      const collapsed = Object.keys(collapsedProjects).filter(
        (projectId) => collapsedProjects[projectId],
      );
      localStorage.setItem(COLLAPSED_PROJECTS_KEY, JSON.stringify(collapsed));
    } catch {
      // Ignore quota/storage errors; collapsing still works for this render.
    }
  }

  const currentPath = $derived(page.url.pathname);
  const activeSessionId = $derived(
    currentPath.startsWith("/sessions/") ? currentPath.replace("/sessions/", "") : null,
  );

  function toggleProject(projectId: string) {
    collapsedProjects[projectId] = !collapsedProjects[projectId];
    saveCollapsedProjects();
  }

  /** The navigation strip stays compact, so every destination keeps a short label. */
  function navItemClass(active: boolean): string {
    return cn(
      "flex min-w-0 flex-1 flex-col items-center gap-0.5 rounded-md px-0 py-1 text-[11px] leading-tight font-medium transition-colors",
      active
        ? "bg-signal-soft text-foreground"
        : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
    );
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

  /**
   * The attention strip is a filter over the session list, not a second copy
   * of it. Typing in the sidebar search leaves the filter so results stay
   * predictable.
   */
  const attentionFilterActive = $derived(
    attentionOnly && needsAttentionSessions.length > 0 && searchQuery.trim() === "",
  );

  interface ProjectSessionGroup {
    projectId: string;
    projectName: string;
    sessions: SessionSummary[];
    pendingCount: number;
    runningCount: number;
    /** Whether the project has any session server-side (loaded page or not) */
    hasSessions: boolean;
  }

  // Session groups by project
  const projectGroups = $derived.by(() => {
    const groups: ProjectSessionGroup[] = [];
    const projectMap = new Map<string, string>();
    const projectsWithSessions = new Set<string>();

    for (const p of sync.projects) {
      projectMap.set(p.project_id, p.name);
      if (p.has_sessions) projectsWithSessions.add(p.project_id);
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
        hasSessions: projectsWithSessions.has(pId) || projSessions.length > 0,
      });
    }

    // Sort projects with the most recently updated sessions first
    return groups.toSorted((a, b) => {
      const aLatest = a.sessions[0]?.updated_at ?? 0;
      const bLatest = b.sessions[0]?.updated_at ?? 0;
      return bLatest - aLatest;
    });
  });

  /**
   * Suppress the fold transition for the first batch of projects. Without this
   * gate, every expanded project would slide open together on initial load.
   * Later fold toggles animate normally.
   */
  let projectsReady = $state(false);
  let projectsSeen = false;

  $effect(() => {
    if (projectsSeen || projectGroups.length === 0) return;
    projectsSeen = true;
    requestAnimationFrame(() => {
      projectsReady = true;
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

  <!-- Quick navigation. One compact strip keeps the session list the
       dominant part of the sidebar. -->
  <nav class="flex items-stretch gap-0.5 border-b border-border p-1">
    <a
      href="/"
      onclick={handleLinkClick}
      title={m.shell_new_session()}
      aria-current={currentPath === "/" ? "page" : undefined}
      class={navItemClass(currentPath === "/")}
    >
      <PlusIcon class="size-4" />
      <span class="w-full truncate text-center">{m.shell_nav_new()}</span>
    </a>

    <a
      href="/projects"
      onclick={handleLinkClick}
      title={m.shell_projects()}
      aria-current={currentPath.startsWith("/projects") ? "page" : undefined}
      class={navItemClass(currentPath.startsWith("/projects"))}
    >
      <FolderGit2Icon class="size-4" />
      <span class="w-full truncate text-center">{m.shell_projects()}</span>
    </a>

    <a
      href="/external"
      onclick={handleLinkClick}
      title={m.shell_external_sessions()}
      aria-current={currentPath === "/external" ? "page" : undefined}
      class={navItemClass(currentPath === "/external")}
    >
      <HistoryIcon class="size-4" />
      <span class="w-full truncate text-center">{m.shell_nav_external()}</span>
    </a>

    <a
      href="/search"
      onclick={handleLinkClick}
      title={m.shell_full_text_search()}
      aria-current={currentPath.startsWith("/search") ? "page" : undefined}
      class={navItemClass(currentPath.startsWith("/search"))}
    >
      <SearchIcon class="size-4" />
      <span class="w-full truncate text-center">{m.shell_nav_search()}</span>
    </a>

    <a
      href="/settings"
      onclick={handleLinkClick}
      title={m.shell_settings()}
      aria-current={currentPath.startsWith("/settings") ? "page" : undefined}
      class={navItemClass(currentPath.startsWith("/settings"))}
    >
      <SettingsIcon class="size-4" />
      <span class="w-full truncate text-center">{m.shell_settings()}</span>
    </a>
  </nav>

  <!-- Search bar -->
  <div class="border-b px-2 py-2">
    <div class="relative flex items-center">
      <SearchIcon class="text-muted-foreground absolute left-2 size-4 pointer-events-none" />
      <input
        type="text"
        bind:value={searchQuery}
        oninput={() => (attentionOnly = false)}
        placeholder={m.shell_search_placeholder()}
        aria-label={m.shell_search_placeholder()}
        class="bg-chip placeholder:text-muted-foreground focus-visible:ring-ring/40 h-8 w-full rounded-md border border-border pr-2 pl-7 text-[13px] outline-none focus-visible:ring-2"
      />
    </div>
  </div>

  <!-- Session list (grouped by project) -->
  <div class="scrollbar-thin flex-1 overflow-y-auto p-2">
    <!-- Attention is a filter over the list, not a second copy of it: the
         same session must not appear twice. -->
    {#if needsAttentionSessions.length > 0 && searchQuery.trim() === ""}
      <button
        type="button"
        class={cn(
          "mb-2 flex h-7 w-full items-center gap-1.5 rounded-md border px-2 text-[11px] font-semibold transition-colors select-none",
          attentionFilterActive
            ? "border-destructive/40 bg-destructive/15 text-destructive"
            : "border-destructive/25 bg-destructive/5 text-destructive hover:bg-destructive/10",
        )}
        aria-pressed={attentionFilterActive}
        title={attentionFilterActive ? m.shell_clear_filter() : m.shell_attention_filter_hint()}
        onclick={() => (attentionOnly = !attentionOnly)}
      >
        <AlertCircleIcon class="size-3 shrink-0" />
        <span class="truncate">{m.shell_needs_attention()}</span>
        <span
          class="rounded-full bg-destructive/15 px-1.5 text-[10px] leading-tight tabular-nums text-destructive"
        >
          {needsAttentionSessions.length}
        </span>
        <span class="ml-auto flex shrink-0 items-center gap-1 text-[10px] font-medium">
          {#if attentionFilterActive}
            {m.shell_clear_filter()}
            <XIcon class="size-3" />
          {:else}
            <ChevronRightIcon class="size-3" />
          {/if}
        </span>
      </button>
    {/if}

    {#if attentionFilterActive}
      <ul class="flex list-none flex-col gap-0.5 p-0">
        {#each needsAttentionSessions as session (session.session_id)}
          <li animate:flip={{ duration: motion.duration(180) }}>
            {@render sessionRow(session)}
          </li>
        {/each}
      </ul>
    {:else if projectGroups.length === 0}
      <div class="text-muted-foreground p-3 text-center text-[13px]">
        {searchQuery ? m.shell_no_matching_results() : m.shell_no_projects_yet()}
      </div>
    {:else}
      <div class="flex flex-col gap-2.5">
        {#each projectGroups as group (group.projectId)}
          {@const isCollapsed = isProjectCollapsed(group.projectId)}
          <div animate:flip={{ duration: motion.duration(180) }} class="flex flex-col">
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
                transition:slide|local={{ duration: projectsReady ? motion.duration(160) : 0 }}
                class="ml-2.5 border-l border-border/60 pl-2 flex list-none flex-col gap-0.5 p-0 mt-0.5"
              >
                {#each group.sessions as session (session.session_id)}
                  <li animate:flip={{ duration: motion.duration(180) }}>
                    {@render sessionRow(session)}
                  </li>
                {/each}
                {#if group.sessions.length === 0 && !sync.hasLoadedProjectSessions(group.projectId)}
                  <li class="py-1 px-1.5">
                    {#if group.hasSessions}
                      {@render loadSessionsButton(group.projectId, m.shell_load_sessions())}
                    {:else}
                      {@render startSessionLink(group.projectId)}
                    {/if}
                  </li>
                {:else if sync.hasMoreSessions(group.projectId)}
                  <li class="py-1 px-1.5">
                    {@render loadSessionsButton(group.projectId, m.shell_load_older_sessions())}
                  </li>
                {:else if group.sessions.length === 0}
                  <li class="py-1 px-1.5 text-[11px] text-muted-foreground">
                    {@render startSessionLink(group.projectId)}
                  </li>
                {/if}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    <!-- Archived sessions (collapsible; hidden while the attention filter owns
         the list, since that list already includes archived waiting sessions) -->
    {#if archivedSessions.length > 0 && !attentionFilterActive}
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
          <ul
            transition:slide|local={{ duration: motion.duration(160) }}
            class="mt-1 flex list-none flex-col gap-0.5 p-0"
          >
            {#each archivedSessions as session (session.session_id)}
              <li animate:flip={{ duration: motion.duration(180) }}>
                {@render sessionRow(session)}
              </li>
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

{#snippet loadSessionsButton(projectId: string, label: string)}
  <button
    type="button"
    class="text-muted-foreground hover:text-foreground hover:bg-accent flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[11px] select-none disabled:opacity-60"
    disabled={sync.sessionsLoadingMore(projectId)}
    onclick={() => void sync.loadMoreSessions(projectId)}
  >
    {#if sync.sessionsLoadingMore(projectId)}
      <LoaderCircleIcon class="size-3 animate-spin" />
    {/if}
    {label}
  </button>
{/snippet}

{#snippet startSessionLink(projectId: string)}
  <a
    href={`/?project=${encodeURIComponent(projectId)}`}
    onclick={handleLinkClick}
    class="hover:underline flex items-center gap-1 text-[11px] text-primary/80"
  >
    <PlusIcon class="size-3" />
    <span>{m.shell_start_session()}</span>
  </a>
{/snippet}

{#snippet sessionRow(session: SessionSummary)}
  {@const isActive = activeSessionId === session.session_id}
  {@const isArchived = session.archived_at !== null}
  {@const isWaiting = isWaitingSessionStatus(session.status)}
  {@const isAttention = sessionNeedsAttention(session)}
  {@const attentionLabel = isAttention ? sessionAttentionBadgeLabel(session.status) : null}
  {@const node = getNode(session.node_id)}
  {@const nodeAvail = getNodeAvailability(node, sync.wsConnected)}
  {@const isPinned = pinned.isPinned(session.session_id)}
  {@const isUnread = sync.hasUnread(session.session_id)}
  <Tooltip.Provider delayDuration={400}>
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <a
            {...props}
            href={`/sessions/${session.session_id}`}
            onclick={handleLinkClick}
            class={cn(
              "session-row group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-[13px] transition-colors",
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
            <!-- Node health is only worth a dot when the node cannot serve
                   the session; healthy rows stay visually quiet. -->
            {#if !nodeAvail.isAvailable}
              <span class={cn("size-1.5 shrink-0 rounded-full", nodeAvail.dotColorClass)}></span>
            {/if}

            <!-- Completed agent output the user has not opened yet -->
            {#if isUnread}
              <span
                class="bg-signal size-1.5 shrink-0 rounded-full"
                role="img"
                title={m.shell_unread()}
                aria-label={m.shell_unread()}
              ></span>
            {/if}

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
              <!-- Resting metadata. Hidden while the actions are open on
                     pointer devices so the two clusters never take horizontal
                     space at the same time; touch screens show both. -->
              <span class="session-row-meta flex items-center gap-1.5">
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

              <!-- Actions. Collapsed until hover/focus on pointer devices so
                     they do not truncate the title at rest; always visible on
                     touch screens (see `.session-row-actions` in app.css). -->
              <div
                class="session-row-actions flex shrink-0 items-center gap-1 overflow-hidden transition-opacity"
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
{/snippet}
