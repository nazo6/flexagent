<script lang="ts">
  import SessionInfo from "#lib/components/session/SessionInfo.svelte";
  import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.svelte";
  import SessionStatusBadge from "#lib/components/SessionStatusBadge.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import type {
    NodeSummary,
    SessionStatus,
    SessionSummary,
    SessionUsage,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sessionNeedsAttention } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import { DOCK_LAUNCHERS, type DockToolId } from "#lib/dock/registry.ts";
  import { cn } from "#lib/utils.ts";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PanelRightOpenIcon from "@lucide/svelte/icons/panel-right-open";

  interface Props {
    session: SessionSummary | null;
    node: NodeSummary | null;
    status: SessionStatus | null;
    usage: SessionUsage | null;
    projectName: string | null;
    canResume: boolean;
    onResume: () => void;
    canNewSessionHere: boolean;
    onNewSessionHere: () => void;
    onForceStop: () => void;
    /** Active dock tool (null while the dock is closed). */
    activeTool: DockToolId | null;
    onToggleTool: (id: DockToolId) => void;
    onDeleted: () => void;
  }

  let {
    session,
    node,
    status,
    usage,
    projectName,
    canResume,
    onResume,
    canNewSessionHere,
    onNewSessionHere,
    onForceStop,
    activeTool,
    onToggleTool,
    onDeleted,
  }: Props = $props();

  const otherAttentionCount = $derived(
    sync.sessions.filter((s) => s.session_id !== session?.session_id && sessionNeedsAttention(s))
      .length,
  );
</script>

<header class="wco-titlebar bg-card/85 border-border shrink-0 border-b backdrop-blur">
  <!-- In an installed desktop PWA this row also serves as the window title bar
       (Window Controls Overlay); see the `.wco-*` rules in app.css. -->
  <div class="wco-titlebar-row flex h-12 items-center gap-2 px-3 lg:px-4">
    <!-- Mobile: open the session drawer -->
    <button
      type="button"
      class="hover:bg-accent text-muted-foreground hover:text-foreground relative flex size-7 shrink-0 items-center justify-center rounded-md border lg:hidden"
      onclick={() => layoutState.setMobileDrawer(true)}
      title={m.shell_open_menu()}
      aria-label={m.shell_open_menu()}
    >
      <MenuIcon class="size-4" />
      {#if otherAttentionCount > 0}
        <span
          class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold"
        >
          {otherAttentionCount > 99 ? "99+" : otherAttentionCount}
        </span>
      {/if}
    </button>

    <!-- Desktop: restore the collapsed sidebar -->
    {#if layoutState.sidebarCollapsed}
      <button
        type="button"
        class="hover:bg-accent text-muted-foreground hover:text-foreground relative hidden size-7 shrink-0 items-center justify-center rounded-md border lg:flex"
        onclick={() => layoutState.toggleSidebar()}
        title={m.shell_show_sidebar()}
        aria-label={m.shell_show_sidebar()}
      >
        <PanelLeftOpenIcon class="size-4" />
        {#if otherAttentionCount > 0}
          <span
            class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold"
          >
            {otherAttentionCount > 99 ? "99+" : otherAttentionCount}
          </span>
        {/if}
      </button>
    {/if}

    <div class="flex min-w-0 flex-1 items-center gap-2">
      <h1 class="min-w-0 truncate text-[15px] font-semibold tracking-tight">
        {session?.title ?? m.session_title_fallback()}
      </h1>
      {#if status}
        <SessionStatusBadge {status} />
      {/if}
    </div>

    <div class="hidden min-w-0 md:flex">
      <SessionInfo {session} {node} {status} {usage} {projectName} />
    </div>

    <div class="flex shrink-0 items-center gap-1">
      <!-- On desktop the sidebar footer owns the badge; when the sidebar is
           collapsed this header is the only remaining place for it. -->
      <ProblemBadge
        class={layoutState.sidebarCollapsed ? "size-7 px-0" : "size-7 px-0 lg:hidden"}
      />
      {#if session}
        <div
          class="border-border flex items-center rounded-md border p-0.5 lg:hidden"
          role="group"
          aria-label={m.session_tools_title()}
        >
          {#each DOCK_LAUNCHERS as tool (tool.id)}
            {@const Icon = tool.icon}
            <button
              type="button"
              class={cn(
                "flex size-6 items-center justify-center rounded-sm transition-colors",
                activeTool === tool.id
                  ? "bg-signal-soft text-signal"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
              title={tool.label()}
              aria-label={tool.label()}
              aria-pressed={activeTool === tool.id}
              onclick={() => onToggleTool(tool.id)}
            >
              <Icon class="size-3.5" />
            </button>
          {/each}
        </div>
      {/if}

      {#if layoutState.rightRailCollapsed}
        <button
          type="button"
          class="hover:bg-accent text-muted-foreground hover:text-foreground hidden size-7 items-center justify-center rounded-md border lg:flex"
          onclick={() => layoutState.setRightRailCollapsed(false)}
          title={m.session_tools_show()}
          aria-label={m.session_tools_show()}
        >
          <PanelRightOpenIcon class="size-4" />
        </button>
      {/if}

      {#if session}
        <SessionActionsMenu
          {session}
          scope="session"
          {canNewSessionHere}
          {onNewSessionHere}
          {canResume}
          {onResume}
          {onForceStop}
          {onDeleted}
          buttonClass="size-7 rounded-md border border-border"
          iconClass="size-4"
        />
      {/if}
    </div>
  </div>

  <!-- Mobile: compact context row -->
  <div class="flex px-3 pb-1.5 md:hidden">
    <SessionInfo {session} {node} {status} {usage} {projectName} class="w-full justify-start" />
  </div>
</header>
