import type { JSX } from "@solidjs/web";
import { useLocation, useNavigate } from "@solidjs/router";
import { createMemo, Match, onSettled, Show, Switch } from "solid-js";
import type { RecoveryAction } from "#lib/generated/bindings.ts";
import AppSidebar from "#lib/components/layout/AppSidebar.tsx";
import BrandIcon from "#lib/components/BrandIcon.tsx";
import ConnectionMenu from "#lib/components/ConnectionMenu.tsx";
import ProblemBadge from "#lib/components/ProblemBadge.tsx";
import ProblemCenter from "#lib/components/ProblemCenter.tsx";
import TokenDialog from "#lib/components/TokenDialog.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import * as Sheet from "#lib/components/ui/sheet/index.ts";
import { Toaster } from "#lib/components/ui/sonner/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { sessionNeedsAttention } from "#lib/session-status.ts";
import { nodeSettingsPath, resolveDefaultNodeId, settingsHomePath } from "#lib/settings-routes.ts";
import { connection, initApp, sync } from "#lib/stores/app.ts";
import { layoutState } from "#lib/stores/layout.ts";
import { problems, type AppProblem } from "#lib/stores/problems.ts";
import { theme } from "#lib/theme.ts";
// Applies the `.dark` class from the stored mode before the first paint; the
// module has no component of its own (`mode-watcher`'s `<ModeWatcher/>` is gone).
import "#lib/mode.ts";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconMenu from "~icons/lucide/menu";
import IconPanelLeftOpen from "~icons/lucide/panel-left-open";
import IconPlugZap from "~icons/lucide/plug-zap";
import IconPlus from "~icons/lucide/plus";
import IconSearch from "~icons/lucide/search";

/** Re-probes the server after the "unreachable" screen's retry button. */
const retry = async (): Promise<void> => {
  await initApp();
};

const onUnhandledRejection = (event: PromiseRejectionEvent): void => {
  problems.report("unhandled error", event.reason);
};

const onUncaughtError = (event: ErrorEvent): void => {
  problems.report("uncaught error", event.error ?? event.message);
};

const onGlobalKeydown = (event: KeyboardEvent): void => {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "b") {
    // Avoid intercepting if focus is in an input or textarea with standard text
    const target = event.target;
    const isInput =
      target instanceof HTMLElement &&
      (target.tagName === "INPUT" || target.tagName === "TEXTAREA");
    if (!isInput) {
      event.preventDefault();
      layoutState.toggleSidebar();
    }
  }
};

/**
 * App shell, ported from `ui/src/routes/+layout.svelte`.
 *
 * Owns the global chrome: connection gating (loading / unreachable / token),
 * the desktop sidebar, the mobile drawer, the Window Controls Overlay strips,
 * the global keybind and error reporting, plus the toast and problem surfaces.
 */
export default function AppShell(props: { children?: JSX.Element }) {
  const location = useLocation();
  const navigate = useNavigate();

  /** Session pages render their own top chrome so mobile has a single header. */
  const isSessionRoute = createMemo(() => location.pathname.startsWith("/sessions/"));
  /**
   * Routes that fill the viewport and scroll inside themselves. `<main>` never
   * scrolls on these routes, so reserving a scrollbar gutter for it would only
   * leave dead space on the right.
   */
  const mainScrolls = createMemo(
    () => !(location.pathname === "/" || location.pathname.startsWith("/sessions/")),
  );
  const attentionCount = createMemo(() => sync.sessions.filter(sessionNeedsAttention).length);

  /**
   * Executes a server-provided recovery action.
   *
   * Navigation-based actions open the screen where the fix is made; execution
   * of the fix itself stays in that screen (bespoke handlers land with the
   * settings screens).
   */
  const handleProblemAction = (problem: AppProblem, action: RecoveryAction): void => {
    const isHub = connection.role === "hub";
    const defaultNodeId = resolveDefaultNodeId(isHub, sync.nodes, connection.systemInfo?.node_id);
    switch (action.kind) {
      case "retry":
        void sync.refreshAll();
        break;
      case "reconnect_node":
        sync.restart();
        break;
      case "resume_with_replay":
      case "show_logs":
        navigate(`/sessions/${action.session_id}`);
        break;
      case "reinstall_agent":
        navigate(nodeSettingsPath("agents", action.node_id));
        break;
      case "login_agent":
        navigate(nodeSettingsPath("agents", defaultNodeId));
        break;
      case "select_online_node":
        navigate(nodeSettingsPath("general", defaultNodeId));
        break;
      case "open_config":
      case "recreate_worktree":
        navigate(settingsHomePath(isHub, defaultNodeId));
        break;
    }
    problems.dismiss(problem.id);
  };

  onSettled(() => {
    theme.init();
    void initApp();
    window.addEventListener("unhandledrejection", onUnhandledRejection);
    window.addEventListener("error", onUncaughtError);
    window.addEventListener("keydown", onGlobalKeydown);
    return () => {
      window.removeEventListener("unhandledrejection", onUnhandledRejection);
      window.removeEventListener("error", onUncaughtError);
      window.removeEventListener("keydown", onGlobalKeydown);
    };
  });

  return (
    <>
      <Toaster richColors position="top-center" />
      <ProblemCenter onAction={handleProblemAction} />
      <TokenDialog />

      <Switch
        fallback={
          <div
            class={[
              "bg-background text-foreground flex h-svh w-full overflow-hidden",
              { "wco-sidebar-collapsed": layoutState.sidebarCollapsed },
            ]}
          >
            {/* Desktop sidebar (persistent on lg and up) */}
            <Show when={!layoutState.sidebarCollapsed}>
              <div class="hidden lg:flex lg:w-[264px] xl:w-[288px] lg:shrink-0 lg:h-full">
                <AppSidebar />
              </div>
            </Show>

            {/* Mobile drawer (Sheet) */}
            <Sheet.Root
              open={layoutState.mobileDrawerOpen}
              onOpenChange={(open) => layoutState.setMobileDrawer(open)}
            >
              <Sheet.Content side="left" class="p-0 w-80 max-w-[85vw]">
                <AppSidebar onNavigate={() => layoutState.setMobileDrawer(false)} />
              </Sheet.Content>
            </Sheet.Root>

            {/* Main area */}
            <div class="relative flex h-full flex-1 flex-col overflow-hidden">
              {/* Desktop: a floating control reopens the collapsed sidebar without
                  reserving a full-height rail. Session pages place this control in
                  their own header instead, so only one toggle is ever visible. */}
              <Show when={layoutState.sidebarCollapsed && !isSessionRoute()}>
                <div class="pointer-events-none absolute top-1 left-1 z-20 hidden flex-col items-center gap-1.5 lg:flex">
                  <button
                    type="button"
                    class="hover:bg-accent text-muted-foreground hover:text-foreground pointer-events-auto relative flex size-7 items-center justify-center rounded-md border border-border bg-background/85 backdrop-blur"
                    onClick={() => layoutState.setSidebarCollapsed(false)}
                    title={m.shell_show_sidebar()}
                    aria-label={m.shell_show_sidebar()}
                  >
                    <IconPanelLeftOpen class="size-4" />
                    <Show when={attentionCount() > 0}>
                      <span class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold">
                        {attentionCount() > 99 ? "99+" : attentionCount()}
                      </span>
                    </Show>
                  </button>
                  <ProblemBadge class="pointer-events-auto size-7 px-0" />
                </div>
              </Show>

              {/* Mobile top bar (visible below lg; session pages own their header) */}
              <Show when={!isSessionRoute()}>
                <header class="wco-titlebar wco-titlebar-bar bg-background/85 flex shrink-0 items-center justify-between border-b border-border px-3 py-2 backdrop-blur lg:hidden">
                  <button
                    type="button"
                    class="hover:bg-accent relative flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                    onClick={() => layoutState.setMobileDrawer(true)}
                    aria-label={m.shell_open_menu()}
                  >
                    <IconMenu class="size-4" />
                    <Show when={attentionCount() > 0}>
                      <span class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold">
                        {attentionCount() > 99 ? "99+" : attentionCount()}
                      </span>
                    </Show>
                  </button>

                  <div class="flex min-w-0 items-center gap-2">
                    <a href="/" class="flex items-center gap-1.5 font-semibold text-sm">
                      <BrandIcon class="size-6" />
                      <span>FlexAgent</span>
                    </a>
                    <ConnectionMenu variant="scope-compact" align="end" />
                  </div>

                  <div class="flex items-center gap-1">
                    <ProblemBadge class="size-8 px-0" />
                    <Show
                      when={location.pathname === "/"}
                      fallback={
                        <a
                          href="/"
                          class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                          title={m.shell_new_session()}
                          aria-label={m.shell_new_session()}
                        >
                          <IconPlus class="size-4" />
                        </a>
                      }
                    >
                      <a
                        href="/search"
                        class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                        title={m.shell_full_text_search()}
                        aria-label={m.shell_full_text_search()}
                      >
                        <IconSearch class="size-4" />
                      </a>
                    </Show>
                  </div>
                </header>

                {/* Desktop Window Controls Overlay strip. Session pages use their own
                    header as the title bar, so this only fills the strip for the shell
                    routes and keeps the native controls off the content. */}
                <div
                  class="wco-titlebar wco-main-titlebar hidden shrink-0 border-b border-border bg-background/85 backdrop-blur"
                  aria-hidden="true"
                />
              </Show>

              <main class={["relative flex-1 overflow-y-auto", { "app-scroll": mainScrolls() }]}>
                {props.children}
              </main>
            </div>
          </div>
        }
      >
        <Match when={!connection.ready}>
          <div class="bg-background text-muted-foreground flex min-h-svh flex-col">
            <WcoDragStrip />
            <div class="flex flex-1 items-center justify-center gap-2">
              <IconLoaderCircle class="size-4 animate-spin" />
              {m.shell_checking_connection()}
            </div>
          </div>
        </Match>

        <Match when={connection.error !== null}>
          <div class="bg-background flex min-h-svh flex-col">
            <WcoDragStrip />
            <div class="flex flex-1 items-center justify-center p-6">
              <div class="flex max-w-md flex-col items-center gap-3 text-center">
                <IconPlugZap class="text-destructive size-8" />
                <h1 class="text-lg font-semibold">{m.shell_server_unreachable()}</h1>
                <p class="text-muted-foreground text-sm">{connection.error}</p>
                <p class="text-muted-foreground text-[13px]">
                  {m.shell_server_down_hint()}
                  <code>{connection.origin}</code>
                  {m.shell_server_down_hint_suffix()}
                </p>
                <div class="flex items-center gap-2">
                  <Button variant="outline" onClick={retry}>
                    {m.common_retry()}
                  </Button>
                  <ConnectionMenu />
                </div>
              </div>
            </div>
          </div>
        </Match>

        <Match when={connection.tokenRequired}>
          <div class="bg-background flex min-h-svh flex-col">
            <WcoDragStrip />
          </div>
        </Match>
      </Switch>
    </>
  );
}

/**
 * Window Controls Overlay removes the OS title bar, so the pre-shell screens
 * need their own drag region while the app connects.
 */
function WcoDragStrip() {
  return (
    <div class="wco-titlebar wco-fallback-titlebar bg-background shrink-0" aria-hidden="true" />
  );
}
