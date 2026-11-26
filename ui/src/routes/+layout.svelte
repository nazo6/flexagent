<script lang="ts">
  import { beforeNavigate, goto } from "$app/navigation";
  import { onMount } from "svelte";
  import "../app.css";
  import AppSidebar from "#lib/components/layout/AppSidebar.svelte";
  import MobileDrawer from "#lib/components/layout/MobileDrawer.svelte";
  import BrandIcon from "#lib/components/BrandIcon.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import ProblemCenter from "#lib/components/ProblemCenter.svelte";
  import TokenDialog from "#lib/components/TokenDialog.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Toaster } from "#lib/components/ui/sonner/index.ts";
  import { connection, initApp, sync } from "#lib/stores/app.svelte.ts";
  import { problems, type AppProblem } from "#lib/stores/problems.svelte.ts";
  import { motion } from "#lib/motion.svelte.ts";
  import { theme } from "#lib/theme.svelte.ts";
  import { initViewport } from "#lib/viewport.ts";
  import { closeTopOverlay } from "#lib/overlays.ts";
  import { uiTraits } from "#lib/ui-traits.svelte.ts";
  import type { RecoveryAction } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { ModeWatcher } from "mode-watcher";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PlugZapIcon from "@lucide/svelte/icons/plug-zap";
  import PlusIcon from "@lucide/svelte/icons/plus";

  import { page } from "$app/state";
  import { sessionNeedsAttention } from "#lib/session-status.ts";
  import {
    nodeSettingsPath,
    resolveDefaultNodeId,
    settingsHomePath,
  } from "#lib/settings-routes.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import SearchIcon from "@lucide/svelte/icons/search";

  let { children } = $props();

  function onUnhandledRejection(event: PromiseRejectionEvent): void {
    problems.report("unhandled error", event.reason);
  }

  function onUncaughtError(event: Event): void {
    const error = event as ErrorEvent;
    problems.report("uncaught error", error.error ?? error.message);
  }

  function onGlobalKeydown(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "b") {
      // Avoid intercepting if focus is in an input or textarea with standard text
      const target = event.target as HTMLElement | null;
      const isInput = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA";
      if (!isInput) {
        event.preventDefault();
        layoutState.toggleSidebar();
      }
    }
  }

  onMount(() => {
    theme.init();
    motion.init();
    const disposeViewport = initViewport();
    void initApp();
    return disposeViewport;
  });

  /**
   * Close overlays before the system back gesture leaves the current route.
   *
   * SvelteKit restores the history entry when a popstate navigation is
   * cancelled, so this behaves like a native modal stack: Back closes the
   * sheet/dock first, and only the next Back navigates.
   */
  beforeNavigate((navigation) => {
    if (navigation.type !== "popstate") return;
    if (closeTopOverlay()) navigation.cancel();
  });

  // The mobile drawer is an overlay, not a route; its component registers
  // itself with the back stack. A desktop resize closes it because the
  // desktop sidebar takes over.
  $effect(() => {
    if (uiTraits.overlayNav) return;
    if (layoutState.mobileDrawerOpen) layoutState.setMobileDrawer(false);
  });

  /**
   * Executes a server-provided recovery action.
   *
   * Navigation-based actions open the screen where the fix is made; execution
   * of the fix itself stays in that screen (Phase 5 will add bespoke handlers).
   */
  function handleProblemAction(problem: AppProblem, action: RecoveryAction): void {
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
        void goto(`/sessions/${action.session_id}`);
        break;
      case "reinstall_agent":
        void goto(nodeSettingsPath("agents", action.node_id));
        break;
      case "login_agent":
        void goto(
          `${nodeSettingsPath("agents", defaultNodeId)}?login=${encodeURIComponent(action.agent_id)}`,
        );
        break;
      case "select_online_node":
        void goto(nodeSettingsPath("general", defaultNodeId));
        break;
      case "open_config":
      case "recreate_worktree":
        void goto(settingsHomePath(isHub, defaultNodeId));
        break;
    }
    problems.dismiss(problem.id);
  }

  async function retry() {
    await initApp();
  }

  /**
   * Routes that fill the viewport and scroll inside themselves. `<main>` never
   * scrolls on these routes, so reserving a scrollbar gutter for it would only
   * leave dead space on the right.
   */
  const SELF_SCROLLING_ROUTES = new Set(["/", "/sessions/[id]"]);
  const mainScrolls = $derived(!SELF_SCROLLING_ROUTES.has(page.route.id ?? ""));
  /** Session pages render their own top chrome so mobile has a single header. */
  const isSessionRoute = $derived(page.route.id === "/sessions/[id]");
  const attentionCount = $derived(sync.sessions.filter(sessionNeedsAttention).length);
</script>

<ModeWatcher />
<svelte:window
  onerror={onUncaughtError}
  onunhandledrejection={onUnhandledRejection}
  onkeydown={onGlobalKeydown}
/>
<Toaster richColors position="top-center" offset="calc(var(--safe-top) + 1rem)" />
<ProblemCenter onaction={handleProblemAction} />
<TokenDialog />

{#snippet wcoDragStrip()}
  <!-- Window Controls Overlay removes the OS title bar, so the pre-shell
       screens need their own drag region while the app connects. -->
  <div class="wco-titlebar wco-fallback-titlebar bg-background shrink-0" aria-hidden="true"></div>
{/snippet}

{#if !connection.ready}
  <div class="safe-area-shell bg-background text-muted-foreground flex min-h-svh flex-col">
    {@render wcoDragStrip()}
    <div class="flex flex-1 flex-col items-center justify-center gap-3">
      <BrandIcon class="size-14" draw />
      <div class="text-muted-foreground flex items-center gap-2">
        <LoaderCircleIcon class="size-4 animate-spin" />
        {m.shell_checking_connection()}
      </div>
    </div>
  </div>
{:else if connection.error !== null}
  <div class="safe-area-shell bg-background flex min-h-svh flex-col">
    {@render wcoDragStrip()}
    <div class="flex flex-1 items-center justify-center p-6">
      <div class="flex max-w-md flex-col items-center gap-3 text-center">
        <PlugZapIcon class="text-destructive size-8" />
        <h1 class="text-lg font-semibold">{m.shell_server_unreachable()}</h1>
        <p class="text-muted-foreground text-sm">{connection.error}</p>
        <p class="text-muted-foreground text-[13px]">
          {m.shell_server_down_hint()}
          <code>{connection.origin}</code>
          {m.shell_server_down_hint_suffix()}
        </p>
        <div class="flex items-center gap-2">
          <Button variant="outline" onclick={retry}>{m.common_retry()}</Button>
          <ConnectionMenu />
        </div>
      </div>
    </div>
  </div>
{:else if connection.tokenRequired}
  <div class="safe-area-shell bg-background flex min-h-svh flex-col">
    {@render wcoDragStrip()}
  </div>
{:else}
  <div
    class="safe-area-shell app-viewport bg-background text-foreground flex w-full overflow-hidden"
    class:wco-sidebar-collapsed={layoutState.sidebarCollapsed}
  >
    <!-- Desktop sidebar (persistent on lg and up) -->
    {#if !uiTraits.overlayNav && !layoutState.sidebarCollapsed}
      <div class="flex h-full w-[264px] shrink-0 xl:w-[288px]">
        <AppSidebar />
      </div>
    {/if}

    <!-- Main area -->
    <div
      class="relative flex h-full flex-1 flex-col overflow-hidden"
      inert={uiTraits.overlayNav && layoutState.mobileDrawerOpen}
    >
      <!-- Desktop: a floating control reopens the collapsed sidebar without
           reserving a full-height rail. Session pages place this control in
           their own header instead, so only one toggle is ever visible. -->
      {#if layoutState.sidebarCollapsed && !isSessionRoute}
        <div
          class="pointer-events-none absolute top-1 left-1 z-20 hidden flex-col items-center gap-1.5 lg:flex"
        >
          <button
            type="button"
            class="hover:bg-accent text-muted-foreground hover:text-foreground pointer-events-auto relative flex size-7 items-center justify-center rounded-md border border-border bg-background/85 backdrop-blur"
            onclick={() => layoutState.setSidebarCollapsed(false)}
            title={m.shell_show_sidebar()}
            aria-label={m.shell_show_sidebar()}
          >
            <PanelLeftOpenIcon class="size-4" />
            {#if attentionCount > 0}
              <span
                class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold"
              >
                {attentionCount > 99 ? "99+" : attentionCount}
              </span>
            {/if}
          </button>
          <ProblemBadge class="pointer-events-auto size-7 px-0" />
        </div>
      {/if}

      <!-- Mobile top bar (visible below lg; session pages own their header) -->
      {#if !isSessionRoute}
        <header
          class="wco-titlebar wco-titlebar-bar bg-background/85 flex shrink-0 items-center justify-between border-b border-border px-3 py-2 backdrop-blur lg:hidden"
        >
          <button
            type="button"
            class="hover:bg-accent touch-target relative flex size-8 items-center justify-center rounded-md border text-muted-foreground"
            onclick={() => layoutState.setMobileDrawer(true)}
            aria-label={m.shell_open_menu()}
          >
            <MenuIcon class="size-4" />
            {#if attentionCount > 0}
              <span
                class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] leading-none font-bold"
              >
                {attentionCount > 99 ? "99+" : attentionCount}
              </span>
            {/if}
          </button>

          <div class="flex min-w-0 items-center gap-2">
            <a href="/" class="flex items-center gap-1.5 font-semibold text-sm">
              <BrandIcon class="size-6" />
              <span>FlexAgent</span>
            </a>
            <ConnectionMenu variant="scope-compact" align="end" />
          </div>

          <div class="flex items-center gap-1">
            <ProblemBadge class="size-8 px-0 touch-target" />
            {#if page.url.pathname === "/"}
              <a
                href="/search"
                class="hover:bg-accent touch-target flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                title={m.shell_full_text_search()}
                aria-label={m.shell_full_text_search()}
              >
                <SearchIcon class="size-4" />
              </a>
            {:else}
              <a
                href="/"
                class="hover:bg-accent touch-target flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                title={m.shell_new_session()}
                aria-label={m.shell_new_session()}
              >
                <PlusIcon class="size-4" />
              </a>
            {/if}
          </div>
        </header>

        <!-- Desktop Window Controls Overlay strip. Session pages use their own
             header as the title bar, so this only fills the strip for the shell
             routes and keeps the native controls off the content. -->
        <div
          class="wco-titlebar wco-main-titlebar hidden shrink-0 border-b border-border bg-background/85 backdrop-blur"
          aria-hidden="true"
        ></div>
      {/if}

      <main class="relative flex-1 overflow-y-auto" class:app-scroll={mainScrolls}>
        {@render children()}
      </main>
    </div>

    {#if uiTraits.overlayNav}
      <MobileDrawer
        open={layoutState.mobileDrawerOpen}
        onopen={() => layoutState.setMobileDrawer(true)}
        onclose={() => layoutState.setMobileDrawer(false)}
      />
    {/if}
  </div>
{/if}
