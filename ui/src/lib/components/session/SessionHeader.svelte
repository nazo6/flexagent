<script lang="ts">
  import { untrack } from "svelte";
  import { loadAgentCatalog } from "#lib/agents.ts";
  import AgentUsage from "#lib/components/session/AgentUsage.svelte";
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
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import { uiTraits } from "#lib/ui-traits.svelte.ts";
  import { DOCK_LAUNCHERS, type DockLauncherId } from "#lib/dock/registry.ts";
  import { cn } from "#lib/utils.ts";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import ListIcon from "@lucide/svelte/icons/list";
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
    /** Active launcher tool (null while the dock is closed or a file tab is active). */
    activeTool: DockLauncherId | null;
    onToggleTool: (id: DockLauncherId) => void;
    /** Whether the turn outline is open (the chat column owns the overlay). */
    turnsOpen: boolean;
    onToggleTurns: () => void;
    onDeleted: () => void;
    /**
     * The chat transcript asked the compact action row to step out of the way.
     * Bindable so the row can bring itself back before its restore timer fires.
     */
    actionsCollapsed?: boolean;
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
    turnsOpen,
    onToggleTurns,
    onDeleted,
    actionsCollapsed = $bindable(false),
  }: Props = $props();

  /** Problems stay visible even while the transcript is scrolling. */
  const actionsHidden = $derived(actionsCollapsed && problems.problems.length === 0);

  const otherAttentionCount = $derived(
    sync.sessions.filter((s) => s.session_id !== session?.session_id && sessionNeedsAttention(s))
      .length,
  );

  // The quota control is a single instance: in the top bar when the header has
  // room, and in the compact context row on phones (where the top bar is
  // already full). It must not be mounted twice, or both copies fetch usage.
  // The usage control keeps the agent identity visible even before the agent
  // reports limits, so the icon comes from the catalog, not the usage payload.
  let agentIcon = $state<string | null>(null);
  let iconRequest = 0;

  async function loadAgentIcon(nodeId: string, agentId: string): Promise<void> {
    const current = ++iconRequest;
    try {
      const catalog = await loadAgentCatalog(nodeId, () =>
        sync.connection.client.nodeAgentCatalog(nodeId),
      );
      if (current !== iconRequest) return;
      agentIcon = catalog.agents.find((agent) => agent.id === agentId)?.icon ?? null;
    } catch {
      if (current === iconRequest) agentIcon = null;
    }
  }

  $effect(() => {
    const nodeId = session?.node_id ?? "";
    const agentId = session?.agent_id ?? "";
    if (nodeId === "" || agentId === "") {
      agentIcon = null;
      return;
    }
    untrack(() => void loadAgentIcon(nodeId, agentId));
  });
</script>

{#snippet headerActions()}
  <div class="flex shrink-0 items-center gap-1">
    {#if session}
      <button
        type="button"
        class={cn(
          "flex size-7 shrink-0 items-center justify-center rounded-md border transition-colors",
          turnsOpen
            ? "border-signal/40 bg-signal-soft text-signal"
            : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
        )}
        title={m.chat_turns_title()}
        aria-label={m.chat_turns_title()}
        aria-pressed={turnsOpen}
        onclick={onToggleTurns}
      >
        <ListIcon class="size-4" />
      </button>
    {/if}
    <!-- On desktop the sidebar footer owns the badge; when the sidebar is
         collapsed this header is the only remaining place for it. -->
    <ProblemBadge class={layoutState.sidebarCollapsed ? "size-7 px-0" : "size-7 px-0 lg:hidden"} />
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
{/snippet}

<header
  class={cn(
    "wco-titlebar bg-card/85 border-border relative z-20 shrink-0 border-b backdrop-blur",
    !actionsHidden && "max-md:border-transparent",
  )}
>
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
      <h1 class="min-w-0 truncate text-[16px] font-semibold tracking-tight lg:text-[15px]">
        {session?.title ?? m.session_title_fallback()}
      </h1>
      {#if status}
        <span role="status">
          <SessionStatusBadge {status} />
        </span>
      {/if}
    </div>

    {#if actionsHidden}
      <button
        type="button"
        class="hover:bg-accent text-muted-foreground hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md border border-border md:hidden"
        title={m.session_actions_show()}
        aria-label={m.session_actions_show()}
        aria-controls="session-compact-actions"
        aria-expanded={!actionsHidden}
        onclick={() => (actionsCollapsed = false)}
      >
        <MoreHorizontalIcon class="size-4" />
      </button>
    {/if}

    <div class="hidden min-w-0 items-center gap-2 md:flex">
      {#if uiTraits.roomyHeader && session}
        <AgentUsage
          nodeId={session.node_id}
          agentId={session.agent_id}
          localPath={session.local_path}
          icon={agentIcon}
        />
      {/if}
      <SessionInfo {session} {node} {status} {usage} {projectName} />
    </div>

    <div class="hidden shrink-0 items-center gap-1 md:flex">
      {@render headerActions()}
    </div>
  </div>

  <!-- Mobile: compact context row with the session actions. It slides away
       while the transcript scrolls and overlays the top of the timeline, so
       showing it again does not resize the conversation. -->
  <div
    id="session-compact-actions"
    class="absolute inset-x-0 top-12 z-20 h-8 overflow-hidden md:hidden"
    inert={actionsHidden}
    aria-hidden={actionsHidden}
  >
    <div
      class={cn(
        "bg-card flex h-8 min-w-0 items-center gap-2 border-b border-border px-3 pb-1 transition-transform duration-200 ease-out",
        actionsHidden && "-translate-y-full",
      )}
    >
      <div class="flex min-w-0 flex-1 items-center gap-1">
        {#if !uiTraits.roomyHeader && session}
          <AgentUsage
            nodeId={session.node_id}
            agentId={session.agent_id}
            localPath={session.local_path}
            icon={agentIcon}
            class="border-transparent"
          />
        {/if}
        <SessionInfo
          {session}
          {node}
          {status}
          {usage}
          {projectName}
          class="min-w-0 flex-1 justify-start"
        />
      </div>
      {@render headerActions()}
    </div>
  </div>

  {#if status === "running"}
    <span class="motion-running-sweep" aria-hidden="true"></span>
  {/if}
</header>

<style>
  /* A 1px signal line along the header edge while the turn is running. It sits
     above the normal border and never changes layout. */
  .motion-running-sweep {
    position: absolute;
    inset-inline: 0;
    bottom: 0;
    height: 1px;
    overflow: hidden;
    pointer-events: none;
  }

  .motion-running-sweep::after {
    content: "";
    position: absolute;
    inset-block: 0;
    width: 38%;
    background: linear-gradient(90deg, transparent, var(--signal), transparent);
    animation: motion-running-sweep 2.4s linear infinite;
  }

  @keyframes motion-running-sweep {
    from {
      transform: translateX(-110%);
    }
    to {
      transform: translateX(360%);
    }
  }
</style>
