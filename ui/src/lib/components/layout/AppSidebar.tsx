import { useLocation } from "@solidjs/router";
import { createMemo, createSignal, For, Show, Switch, Match } from "solid-js";
import BrandIcon from "#lib/components/BrandIcon.tsx";
import ConnectionMenu from "#lib/components/ConnectionMenu.tsx";
import ProblemBadge from "#lib/components/ProblemBadge.tsx";
import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.tsx";
import ExternalSessionsDialog from "#lib/components/session/ExternalSessionsDialog.tsx";
import { Badge } from "#lib/components/ui/badge/index.ts";
import * as Tooltip from "#lib/components/ui/tooltip/index.ts";
import { formatRelativeTimeCompact } from "#lib/format.ts";
import type { SessionSummary } from "#lib/generated/bindings.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  isWaitingSessionStatus,
  sessionAttentionBadgeLabel,
  sessionNeedsAttention,
  sessionStatusLabel,
} from "#lib/session-status.ts";
import { SETTINGS_ROOT, isSettingsPath } from "#lib/settings-routes.ts";
import { sync } from "#lib/stores/app.ts";
import { layoutState } from "#lib/stores/layout.ts";
import { pinned } from "#lib/stores/pinned.ts";
import { cn } from "#lib/utils.ts";
import IconAlertCircle from "~icons/lucide/alert-circle";
import IconArchive from "~icons/lucide/archive";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronRight from "~icons/lucide/chevron-right";
import IconFolder from "~icons/lucide/folder";
import IconFolderGit2 from "~icons/lucide/folder-git-2";
import IconGitBranch from "~icons/lucide/git-branch";
import IconHistory from "~icons/lucide/history";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPanelLeftClose from "~icons/lucide/panel-left-close";
import IconPin from "~icons/lucide/pin";
import IconPlus from "~icons/lucide/plus";
import IconSearch from "~icons/lucide/search";
import IconSettings from "~icons/lucide/settings";
import IconTriangleAlert from "~icons/lucide/triangle-alert";

/** Case-insensitive match of a session's titles and identifiers against the filter. */
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

/**
 * Application sidebar, ported from
 * `ui/src/lib/components/layout/AppSidebar.svelte`.
 *
 * Quick navigation, a session filter, sessions grouped by project (with the
 * needs-attention and archived sections) and the footer with the problem badge
 * and the connection target selector. The same component serves the desktop
 * rail and the mobile drawer; `onNavigate` closes the drawer after a link.
 */

export interface AppSidebarProps {
  onNavigate?: () => void;
}

interface ProjectSessionGroup {
  /** Stable key for `<For keyed>` (the project ID). */
  key: string;
  projectId: string;
  projectName: string;
  sessions: SessionSummary[];
  pendingCount: number;
  runningCount: number;
}

export default function AppSidebar(props: AppSidebarProps) {
  const location = useLocation();

  const [searchQuery, setSearchQuery] = createSignal("");
  const [collapsedProjects, setCollapsedProjects] = createSignal<Record<string, boolean>>({});
  const [showAttentionSection, setShowAttentionSection] = createSignal(true);
  const [showArchivedSection, setShowArchivedSection] = createSignal(false);
  const [externalOpen, setExternalOpen] = createSignal(false);

  const currentPath = createMemo(() => location.pathname);
  const activeSessionId = createMemo(() =>
    currentPath().startsWith("/sessions/") ? currentPath().replace("/sessions/", "") : null,
  );

  const toggleProject = (projectId: string): void => {
    setCollapsedProjects((current) => ({ ...current, [projectId]: !current[projectId] }));
  };

  const isProjectCollapsed = (projectId: string): boolean => {
    if (searchQuery().trim() !== "") return false;
    return collapsedProjects()[projectId] ?? false;
  };

  const filteredSessions = createMemo(() =>
    sync.sessions
      .filter((session) => session.archived_at === null)
      .filter((session) => matchesSearch(session, searchQuery().trim())),
  );

  // Archived sessions (collapsed by default)
  const archivedSessions = createMemo(() =>
    sync.sessions
      .filter((session) => session.archived_at !== null)
      .filter((session) => matchesSearch(session, searchQuery().trim()))
      .toSorted((a, b) => (b.archived_at ?? 0) - (a.archived_at ?? 0)),
  );

  // Sessions needing attention across all projects (waiting sessions stay visible even when archived)
  const needsAttentionSessions = createMemo(() =>
    sync.sessions
      .filter((session) => sessionNeedsAttention(session))
      .toSorted((a, b) => b.updated_at - a.updated_at),
  );

  // Session groups by project
  const projectGroups = createMemo<ProjectSessionGroup[]>(() => {
    const groups: ProjectSessionGroup[] = [];
    const projectMap = new Map<string, string>();

    for (const p of sync.projects) {
      projectMap.set(p.project_id, p.name);
    }

    const allProjectIds = new Set<string>();
    for (const s of filteredSessions()) {
      allProjectIds.add(s.project_id);
    }
    for (const p of sync.projects) {
      allProjectIds.add(p.project_id);
    }

    for (const pId of allProjectIds) {
      const projSessions = filteredSessions()
        .filter((s) => s.project_id === pId)
        .toSorted((a, b) => {
          const aPinned = pinned.isPinned(a.session_id) ? 1 : 0;
          const bPinned = pinned.isPinned(b.session_id) ? 1 : 0;
          if (aPinned !== bPinned) return bPinned - aPinned;
          return b.updated_at - a.updated_at;
        });

      if (searchQuery().trim() !== "" && projSessions.length === 0) {
        continue;
      }

      const pendingCount = projSessions.filter((s) => isWaitingSessionStatus(s.status)).length;
      const runningCount = projSessions.filter((s) => s.status === "running").length;

      groups.push({
        key: pId,
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

  const handleLinkClick = (): void => {
    props.onNavigate?.();
  };

  return (
    <>
      <aside class="bg-sidebar flex h-full w-full flex-col border-r border-border">
        {/* Sidebar header. In a Window Controls Overlay window this is the left
            half of the title bar; see the `.wco-*` rules in styles.css. */}
        <div class="wco-titlebar wco-sidebar-header flex items-center justify-between border-b border-border px-3.5 py-3">
          <a
            href="/"
            class="flex items-center gap-2.5 font-semibold tracking-tight hover:opacity-80 transition-opacity"
            onClick={handleLinkClick}
          >
            <BrandIcon class="size-7" />
            <span class="text-sm">FlexAgent</span>
          </a>
          <button
            type="button"
            class="hover:bg-accent text-muted-foreground hover:text-foreground hidden size-7 items-center justify-center rounded-md border lg:flex"
            onClick={() => layoutState.toggleSidebar()}
            title={m.shell_hide_sidebar()}
            aria-label={m.shell_hide_sidebar()}
          >
            <IconPanelLeftClose class="size-4" />
          </button>
        </div>

        {/* Quick navigation */}
        <nav class="flex flex-col gap-0.5 border-b border-border p-2 text-[13px]">
          <a
            href="/"
            onClick={handleLinkClick}
            class={cn(
              "flex items-center justify-between rounded-md px-2.5 py-1.5 font-medium transition-colors",
              currentPath() === "/"
                ? "bg-signal-soft text-foreground"
                : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
            )}
          >
            <div class="flex items-center gap-2">
              <IconPlus class="size-4" />
              <span>{m.shell_new_session()}</span>
            </div>
          </a>

          <a
            href="/projects"
            onClick={handleLinkClick}
            class={cn(
              "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
              currentPath().startsWith("/projects")
                ? "bg-signal-soft text-foreground"
                : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
            )}
          >
            <IconFolderGit2 class="size-4" />
            <span>{m.shell_projects()}</span>
          </a>

          <button
            type="button"
            onClick={() => setExternalOpen(true)}
            class="text-muted-foreground hover:bg-accent/50 hover:text-foreground flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left font-medium transition-colors"
          >
            <IconHistory class="size-4" />
            <span>{m.shell_external_sessions()}</span>
          </button>

          <a
            href="/search"
            onClick={handleLinkClick}
            class={cn(
              "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
              currentPath().startsWith("/search")
                ? "bg-signal-soft text-foreground"
                : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
            )}
          >
            <IconSearch class="size-4" />
            <span>{m.shell_full_text_search()}</span>
          </a>

          <a
            href={SETTINGS_ROOT}
            onClick={handleLinkClick}
            class={cn(
              "flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors",
              isSettingsPath(currentPath())
                ? "bg-signal-soft text-foreground"
                : "text-muted-foreground hover:bg-accent/50 hover:text-foreground",
            )}
          >
            <IconSettings class="size-4" />
            <span>{m.shell_settings()}</span>
          </a>
        </nav>

        {/* Search bar */}
        <div class="border-b px-2 py-2">
          <div class="relative flex items-center">
            <IconSearch class="text-muted-foreground absolute left-2 size-4 pointer-events-none" />
            <input
              type="text"
              value={searchQuery()}
              onInput={(event) => setSearchQuery(event.currentTarget.value)}
              placeholder={m.shell_search_placeholder()}
              aria-label={m.shell_search_placeholder()}
              class="bg-chip placeholder:text-muted-foreground focus-visible:ring-ring/40 h-8 w-full rounded-md border border-border pr-2 pl-7 text-[13px] outline-none focus-visible:ring-2"
            />
          </div>
        </div>

        {/* Session list (grouped by project) */}
        <div class="scrollbar-thin flex-1 overflow-y-auto p-2">
          {/* Needs attention section (shown at the top only when there are pending approvals or errors) */}
          <Show when={needsAttentionSessions().length > 0 && searchQuery().trim() === ""}>
            <div class="mb-3 rounded-lg border border-destructive/30 bg-destructive/5 p-1.5">
              <button
                type="button"
                class="text-destructive flex w-full items-center justify-between px-1 py-0.5 text-[11px] font-semibold tracking-wider uppercase select-none"
                onClick={() => setShowAttentionSection((shown) => !shown)}
              >
                <span class="flex items-center gap-1.5">
                  <IconAlertCircle class="size-3" />
                  <span>{m.shell_needs_attention()}</span>
                  <span class="rounded-full bg-destructive/15 px-1.5 py-0.2 text-[11px] text-destructive">
                    {needsAttentionSessions().length}
                  </span>
                </span>
                <Show when={showAttentionSection()} fallback={<IconChevronRight class="size-3" />}>
                  <IconChevronDown class="size-3" />
                </Show>
              </button>

              <Show when={showAttentionSection()}>
                <ul class="flex list-none flex-col gap-0.5 p-0 mt-1">
                  <For each={needsAttentionSessions()} keyed={(session) => session.session_id}>
                    {(session) => (
                      <SessionRow
                        session={session()}
                        activeSessionId={activeSessionId()}
                        onNavigate={handleLinkClick}
                      />
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </Show>

          {/* List by project */}
          <Show
            when={projectGroups().length > 0}
            fallback={
              <div class="text-muted-foreground p-3 text-center text-[13px]">
                {searchQuery() ? m.shell_no_matching_results() : m.shell_no_projects_yet()}
              </div>
            }
          >
            <div class="flex flex-col gap-2.5">
              <For each={projectGroups()} keyed={(group) => group.key}>
                {(group) => (
                  <ProjectGroup
                    group={group()}
                    activeSessionId={activeSessionId()}
                    onNavigate={handleLinkClick}
                    collapsed={isProjectCollapsed(group().projectId)}
                    onToggle={() => toggleProject(group().projectId)}
                  />
                )}
              </For>
            </div>
          </Show>

          {/* Archived sessions (collapsible) */}
          <Show when={archivedSessions().length > 0}>
            <div class="mt-3 border-t pt-2">
              <button
                type="button"
                class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-0.5 text-[11px] font-semibold tracking-wider uppercase select-none"
                onClick={() => setShowArchivedSection((shown) => !shown)}
              >
                <span class="flex items-center gap-1.5">
                  <IconArchive class="size-3" />
                  <span>{m.shell_archived()}</span>
                  <span class="bg-muted rounded-full px-1.5 py-0.2 text-[11px]">
                    {archivedSessions().length}
                  </span>
                </span>
                <Show when={showArchivedSection()} fallback={<IconChevronRight class="size-3" />}>
                  <IconChevronDown class="size-3" />
                </Show>
              </button>

              <Show when={showArchivedSection()}>
                <ul class="mt-1 flex list-none flex-col gap-0.5 p-0">
                  <For each={archivedSessions()} keyed={(session) => session.session_id}>
                    {(session) => (
                      <SessionRow
                        session={session()}
                        activeSessionId={activeSessionId()}
                        onNavigate={handleLinkClick}
                      />
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </Show>
        </div>

        {/* Footer: problems and the connection target selector */}
        <div class="border-border flex flex-col gap-2 border-t p-2">
          <ProblemBadge label class="w-full justify-start" />
          <ConnectionMenu variant="scope" />
        </div>
      </aside>

      <ExternalSessionsDialog open={externalOpen()} onOpenChange={setExternalOpen} />
    </>
  );
}

/**
 * One project section of the sidebar: a collapsible header and its sessions.
 *
 * Rendered from a keyed list, so a WS update that replaces the session objects
 * does not rebuild the section (and the row-owned menus keep their state).
 */
function ProjectGroup(props: {
  group: ProjectSessionGroup;
  activeSessionId: string | null;
  onNavigate: () => void;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <div class="flex flex-col">
      {/* Project header */}
      <div class="group/proj flex items-center justify-between rounded-md px-1.5 py-1 text-[13px] hover:bg-muted/40 transition-colors">
        <button
          type="button"
          class="flex min-w-0 flex-1 items-center gap-1.5 text-left font-semibold text-foreground/90 select-none"
          onClick={() => props.onToggle()}
        >
          <span class="text-muted-foreground shrink-0">
            <Show when={props.collapsed} fallback={<IconChevronDown class="size-4" />}>
              <IconChevronRight class="size-4" />
            </Show>
          </span>
          <IconFolderGit2 class="size-4 text-muted-foreground shrink-0" />
          <span class="truncate">{props.group.projectName}</span>
          <Show when={props.group.pendingCount > 0}>
            <Badge variant="destructive" class="px-1 py-0 text-[11px] leading-tight shrink-0">
              {props.group.pendingCount}
            </Badge>
          </Show>
          <Show when={props.group.runningCount > 0}>
            <span
              class="size-1.5 rounded-full bg-ok shrink-0"
              title={m.shell_has_running_sessions()}
            />
          </Show>
        </button>

        <div class="flex items-center gap-1 shrink-0 ml-1">
          <span class="text-muted-foreground text-[11px]">{props.group.sessions.length}</span>
          <a
            href={`/?project=${encodeURIComponent(props.group.projectId)}`}
            onClick={() => props.onNavigate()}
            class="hover:bg-accent text-muted-foreground/60 hover:text-foreground rounded p-0.5 transition-colors"
            title={m.shell_start_new_session_in_project({ project: props.group.projectName })}
          >
            <IconPlus class="size-3" />
          </a>
        </div>
      </div>

      {/* Sessions in the project */}
      <Show when={!props.collapsed}>
        <ul class="ml-2.5 border-l border-border/60 pl-2 flex list-none flex-col gap-0.5 p-0 mt-0.5">
          <For each={props.group.sessions} keyed={(session) => session.session_id}>
            {(session) => (
              <SessionRow
                session={session()}
                activeSessionId={props.activeSessionId}
                onNavigate={props.onNavigate}
              />
            )}
          </For>
          <Switch>
            <Match
              when={
                props.group.sessions.length === 0 &&
                !sync.hasLoadedProjectSessions(props.group.projectId)
              }
            >
              <li class="py-1 px-1.5">
                <LoadSessionsButton projectId={props.group.projectId} />
              </li>
            </Match>
            <Match when={sync.hasMoreSessions(props.group.projectId)}>
              <li class="py-1 px-1.5">
                <LoadSessionsButton projectId={props.group.projectId} older />
              </li>
            </Match>
            <Match when={props.group.sessions.length === 0}>
              <li class="py-1 px-1.5 text-[11px] text-muted-foreground">
                <a
                  href={`/?project=${encodeURIComponent(props.group.projectId)}`}
                  onClick={() => props.onNavigate()}
                  class="hover:underline flex items-center gap-1 text-primary/80"
                >
                  <IconPlus class="size-3" />
                  <span>{m.shell_start_session()}</span>
                </a>
              </li>
            </Match>
          </Switch>
        </ul>
      </Show>
    </div>
  );
}

/**
 * "Load sessions" row for a project.
 *
 * A project without a cursor shows the first page; with one it appends older
 * sessions. The spinner replaces the icon while the request is in flight.
 */
function LoadSessionsButton(props: { projectId: string; older?: boolean }) {
  return (
    <button
      type="button"
      class="text-muted-foreground hover:text-foreground hover:bg-accent flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[11px] select-none disabled:opacity-60"
      disabled={sync.sessionsLoadingMore(props.projectId)}
      onClick={() => void sync.loadMoreSessions(props.projectId)}
    >
      <Show when={sync.sessionsLoadingMore(props.projectId)}>
        <IconLoaderCircle class="size-3 animate-spin" />
      </Show>
      {props.older === true ? m.shell_load_older_sessions() : m.shell_load_sessions()}
    </button>
  );
}

interface SessionRowProps {
  session: SessionSummary;
  activeSessionId: string | null;
  onNavigate: () => void;
}

/**
 * One session row with its hover actions and the detail tooltip.
 *
 * The tooltip trigger is the link itself (`as="a"`); the wrapper's prop type is
 * button-shaped, so `href` and `onClick` reach the anchor through a spread.
 */
function SessionRow(props: SessionRowProps) {
  const isActive = () => props.activeSessionId === props.session.session_id;
  const isArchived = () => props.session.archived_at !== null;
  const isWaiting = () => isWaitingSessionStatus(props.session.status);
  const isAttention = () => sessionNeedsAttention(props.session);
  const attentionLabel = () =>
    isAttention() ? sessionAttentionBadgeLabel(props.session.status) : null;
  const nodeAvailability = createMemo(() => {
    const node = sync.nodes.find((candidate) => candidate.node_id === props.session.node_id);
    return getNodeAvailability(node, sync.wsConnected);
  });
  const isPinned = () => pinned.isPinned(props.session.session_id);
  const isUnread = () => sync.hasUnread(props.session.session_id);

  return (
    <li>
      <Tooltip.Provider delayDuration={400}>
        <Tooltip.Root>
          <Tooltip.Trigger
            as="a"
            {...{
              href: `/sessions/${props.session.session_id}`,
              onClick: props.onNavigate,
            }}
            class={cn(
              "group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 text-[13px] transition-colors",
              isActive()
                ? "bg-signal-soft text-foreground font-medium"
                : isWaiting()
                  ? "text-foreground font-semibold hover:bg-warn-soft/50"
                  : isAttention()
                    ? "text-foreground font-semibold hover:bg-danger-soft/50"
                    : "text-foreground/80 hover:bg-accent/40 hover:text-foreground",
              isArchived() && !isWaiting() && "opacity-70",
            )}
          >
            {/* Node health is only worth a dot when the node cannot serve
                the session; healthy rows stay visually quiet. */}
            <Show when={!nodeAvailability().isAvailable}>
              <span
                class={cn("size-1.5 shrink-0 rounded-full", nodeAvailability().dotColorClass)}
              />
            </Show>

            {/* Completed agent output the user has not opened yet */}
            <Show when={isUnread()}>
              <span
                class="bg-signal size-1.5 shrink-0 rounded-full"
                role="img"
                title={m.shell_unread()}
                aria-label={m.shell_unread()}
              />
            </Show>

            {/* The agent is answering / working right now */}
            <Show when={props.session.status === "running"}>
              <span
                class="text-signal shrink-0"
                role="img"
                title={m.status_running()}
                aria-label={m.status_running()}
              >
                <IconLoaderCircle class="size-3.5 animate-spin" aria-hidden="true" />
              </span>
            </Show>

            <Show when={isAttention()}>
              <Show
                when={isWaiting()}
                fallback={
                  <IconAlertCircle class="text-danger size-3.5 shrink-0" aria-hidden="true" />
                }
              >
                <IconTriangleAlert class="text-warn size-3.5 shrink-0" aria-hidden="true" />
              </Show>
            </Show>

            {/* Title */}
            <span class="min-w-0 flex-1 truncate">{props.session.title}</span>

            <Show when={attentionLabel() !== null}>
              <span
                class={cn(
                  "shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold leading-none",
                  isWaiting() ? "bg-warn/15 text-warn" : "bg-danger/15 text-danger",
                )}
              >
                {attentionLabel()}
              </span>
            </Show>

            <div class="flex shrink-0 items-center text-muted-foreground text-[11px]">
              {/* Resting metadata. Hidden while the actions are open so the
                  two clusters never take horizontal space at the same time. */}
              <span class="flex items-center gap-1.5 group-hover:hidden group-focus-within:hidden">
                <Show when={isPinned()}>
                  <IconPin
                    class="text-signal size-3 shrink-0"
                    fill="currentColor"
                    aria-hidden="true"
                  />
                </Show>
                <span class="tabular-nums">
                  {formatRelativeTimeCompact(props.session.updated_at)}
                </span>
              </span>

              {/* Actions. Zero-width until hover/focus: they stay focusable
                  from the keyboard but do not truncate the title at rest. */}
              <div class="flex w-0 shrink-0 items-center gap-1 overflow-hidden opacity-0 transition-opacity group-hover:ml-1.5 group-hover:w-auto group-hover:opacity-100 group-focus-within:ml-1.5 group-focus-within:w-auto group-focus-within:opacity-100">
                {/* Archive / delete menu */}
                <SessionActionsMenu session={props.session} align="end" iconClass="size-3.5" />

                {/* Pin button */}
                <button
                  type="button"
                  class={cn(
                    "hover:text-foreground p-0.5 transition-colors",
                    isPinned() && "text-signal",
                  )}
                  title={isPinned() ? m.shell_unpin() : m.shell_pin()}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    pinned.toggle(props.session.session_id);
                  }}
                >
                  <IconPin class="size-3" fill={isPinned() ? "currentColor" : "none"} />
                </button>
              </div>
            </div>
          </Tooltip.Trigger>
          <Tooltip.Content
            side="right"
            align="start"
            class="max-w-[300px] flex-col items-stretch gap-1.5 px-2.5 py-2 text-left leading-snug"
          >
            <span class="font-medium break-words">
              {`${props.session.title}${isArchived() ? ` (${m.shell_archived()})` : ""}`}
            </span>
            <span class="text-background/70 flex items-center gap-1.5">
              <IconFolderGit2 class="size-3 shrink-0" />
              <span class="truncate">{props.session.project_id}</span>
            </span>
            <Show when={props.session.git_branch !== null}>
              <span class="text-background/70 flex items-center gap-1.5">
                <IconGitBranch class="size-3 shrink-0" />
                <span class="truncate font-mono">{props.session.git_branch}</span>
              </span>
            </Show>
            <span class="text-background/70 flex items-start gap-1.5">
              <IconFolder class="mt-0.5 size-3 shrink-0" />
              <span class="font-mono text-[11px] break-all">{props.session.local_path}</span>
            </span>
            <span class="text-background/70 flex items-center gap-1.5">
              <span
                class={cn("size-1.5 shrink-0 rounded-full", nodeAvailability().dotColorClass)}
              />
              <span class="truncate">
                {sessionStatusLabel(props.session.status)}
                {" · "}
                {m.shell_node_status({
                  node: nodeStatusNodeName(props.session.node_id),
                  status: nodeAvailability().statusText,
                })}
              </span>
            </span>
          </Tooltip.Content>
        </Tooltip.Root>
      </Tooltip.Provider>
    </li>
  );
}

/** Node name for the row tooltip (`node_id` when the node is unknown). */
function nodeStatusNodeName(nodeId: string): string {
  return sync.nodes.find((candidate) => candidate.node_id === nodeId)?.name ?? nodeId;
}
