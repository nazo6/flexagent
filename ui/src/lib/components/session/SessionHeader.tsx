import { createEffect, createMemo, createSignal, For, onSettled, Show } from "solid-js";
import AgentUsage from "#lib/components/session/AgentUsage.tsx";
import SessionInfo from "#lib/components/session/SessionInfo.tsx";
import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.tsx";
import SessionStatusBadge from "#lib/components/SessionStatusBadge.tsx";
import ProblemBadge from "#lib/components/ProblemBadge.tsx";
import type {
  NodeSummary,
  SessionStatus,
  SessionSummary,
  SessionUsage,
} from "#lib/generated/bindings.ts";
import { loadAgentCatalog } from "#lib/agents.ts";
import { m } from "#lib/paraglide/messages.js";
import { sessionNeedsAttention } from "#lib/session-status.ts";
import { sync } from "#lib/stores/app.ts";
import { layoutState } from "#lib/stores/layout.ts";
import { DOCK_LAUNCHERS, type DockLauncherId } from "#lib/dock/registry.ts";
import { cn } from "#lib/utils.ts";
import IconMenu from "~icons/lucide/menu";
import IconPanelLeftOpen from "~icons/lucide/panel-left-open";
import IconPanelRightOpen from "~icons/lucide/panel-right-open";

export interface SessionHeaderProps {
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
  onDeleted: () => void;
}

/** Title bar of an open session (identity, tools, actions menu). */
export default function SessionHeader(props: SessionHeaderProps) {
  const otherAttentionCount = () =>
    sync.sessions.filter(
      (s) => s.session_id !== props.session?.session_id && sessionNeedsAttention(s),
    ).length;

  // The quota control is a single instance: in the top bar on desktop and in
  // the compact context row on phones (where the top bar is already full).
  const [isDesktop, setIsDesktop] = createSignal(
    typeof window !== "undefined" && window.innerWidth >= 768,
  );
  const syncDesktop = () => setIsDesktop(typeof window !== "undefined" && window.innerWidth >= 768);

  // The usage control keeps the agent identity visible even before the agent
  // reports limits, so the icon comes from the catalog, not the usage payload.
  const [agentIcon, setAgentIcon] = createSignal<string | null>(null);
  let iconRequest = 0;

  async function loadAgentIcon(nodeId: string, agentId: string): Promise<void> {
    const current = ++iconRequest;
    try {
      const catalog = await loadAgentCatalog(nodeId, () =>
        sync.connection.client.nodeAgentCatalog(nodeId),
      );
      if (current !== iconRequest) return;
      setAgentIcon(catalog.agents.find((agent) => agent.id === agentId)?.icon ?? null);
    } catch {
      if (current === iconRequest) setAgentIcon(null);
    }
  }

  const agentIdentity = createMemo(
    () => `${props.session?.node_id ?? ""}\u0000${props.session?.agent_id ?? ""}`,
  );

  createEffect(agentIdentity, (key) => {
    const [nodeId, agentId] = key.split("\u0000");
    if (nodeId === "" || agentId === "") {
      setAgentIcon(null);
      return;
    }
    void loadAgentIcon(nodeId, agentId);
  });

  // Component-level setup and teardown (not a reactive dependency).
  onSettled(() => {
    if (typeof window === "undefined") return;
    window.addEventListener("resize", syncDesktop);
    return () => window.removeEventListener("resize", syncDesktop);
  });

  /** Attention badge shared by the drawer and sidebar-restore buttons. */
  const attentionBadge = () => (
    <Show when={otherAttentionCount() > 0}>
      <span class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold">
        {otherAttentionCount() > 99 ? "99+" : otherAttentionCount()}
      </span>
    </Show>
  );

  return (
    <header class="wco-titlebar bg-card/85 border-border shrink-0 border-b backdrop-blur">
      {/* In an installed desktop PWA this row also serves as the window title bar
          (Window Controls Overlay); see the `.wco-*` rules in styles.css. */}
      <div class="wco-titlebar-row flex h-12 items-center gap-2 px-3 lg:px-4">
        {/* Mobile: open the session drawer */}
        <button
          type="button"
          class="hover:bg-accent text-muted-foreground hover:text-foreground relative flex size-7 shrink-0 items-center justify-center rounded-md border lg:hidden"
          onClick={() => layoutState.setMobileDrawer(true)}
          title={m.shell_open_menu()}
          aria-label={m.shell_open_menu()}
        >
          <IconMenu class="size-4" />
          {attentionBadge()}
        </button>

        {/* Desktop: restore the collapsed sidebar */}
        <Show when={layoutState.sidebarCollapsed}>
          <button
            type="button"
            class="hover:bg-accent text-muted-foreground hover:text-foreground relative hidden size-7 shrink-0 items-center justify-center rounded-md border lg:flex"
            onClick={() => layoutState.toggleSidebar()}
            title={m.shell_show_sidebar()}
            aria-label={m.shell_show_sidebar()}
          >
            <IconPanelLeftOpen class="size-4" />
            {attentionBadge()}
          </button>
        </Show>

        <div class="flex min-w-0 flex-1 items-center gap-2">
          <h1 class="min-w-0 truncate text-[15px] font-semibold tracking-tight">
            {props.session?.title ?? m.session_title_fallback()}
          </h1>
          <Show when={props.status}>{(status) => <SessionStatusBadge status={status()} />}</Show>
        </div>

        <div class="hidden min-w-0 items-center gap-2 md:flex">
          <Show when={isDesktop() && props.session}>
            {(session) => (
              <AgentUsage
                nodeId={session().node_id}
                agentId={session().agent_id}
                localPath={session().local_path}
                icon={agentIcon()}
              />
            )}
          </Show>
          <SessionInfo
            session={props.session}
            node={props.node}
            status={props.status}
            usage={props.usage}
            projectName={props.projectName}
          />
        </div>

        <div class="flex shrink-0 items-center gap-1">
          {/* On desktop the sidebar footer owns the badge; when the sidebar is
           collapsed this header is the only remaining place for it. */}
          <ProblemBadge
            class={layoutState.sidebarCollapsed ? "size-7 px-0" : "size-7 px-0 lg:hidden"}
          />
          <Show when={props.session}>
            <div
              class="border-border flex items-center rounded-md border p-0.5 lg:hidden"
              role="group"
              aria-label={m.session_tools_title()}
            >
              <For each={DOCK_LAUNCHERS}>
                {(tool) => {
                  const Icon = tool.icon;
                  return (
                    <button
                      type="button"
                      class={cn(
                        "flex size-6 items-center justify-center rounded-sm transition-colors",
                        props.activeTool === tool.id
                          ? "bg-signal-soft text-signal"
                          : "text-muted-foreground hover:bg-accent hover:text-foreground",
                      )}
                      title={tool.label()}
                      aria-label={tool.label()}
                      aria-pressed={props.activeTool === tool.id ? "true" : "false"}
                      onClick={() => props.onToggleTool(tool.id)}
                    >
                      <Icon class="size-3.5" />
                    </button>
                  );
                }}
              </For>
            </div>
          </Show>

          <Show when={layoutState.rightRailCollapsed}>
            <button
              type="button"
              class="hover:bg-accent text-muted-foreground hover:text-foreground hidden size-7 items-center justify-center rounded-md border lg:flex"
              onClick={() => layoutState.setRightRailCollapsed(false)}
              title={m.session_tools_show()}
              aria-label={m.session_tools_show()}
            >
              <IconPanelRightOpen class="size-4" />
            </button>
          </Show>

          <Show when={props.session}>
            {(session) => (
              <SessionActionsMenu
                session={session()}
                scope="session"
                canNewSessionHere={props.canNewSessionHere}
                onNewSessionHere={props.onNewSessionHere}
                canResume={props.canResume}
                onResume={props.onResume}
                onForceStop={props.onForceStop}
                onDeleted={props.onDeleted}
                buttonClass="size-7 rounded-md border border-border"
                iconClass="size-4"
              />
            )}
          </Show>
        </div>
      </div>

      {/* Mobile: compact context row */}
      <div class="flex min-w-0 items-center gap-1 px-3 pb-1.5 md:hidden">
        <Show when={!isDesktop() && props.session}>
          {(session) => (
            <AgentUsage
              nodeId={session().node_id}
              agentId={session().agent_id}
              localPath={session().local_path}
              icon={agentIcon()}
              class="border-transparent"
            />
          )}
        </Show>
        <SessionInfo
          session={props.session}
          node={props.node}
          status={props.status}
          usage={props.usage}
          projectName={props.projectName}
          class="min-w-0 flex-1 justify-start"
        />
      </div>
    </header>
  );
}
