<script lang="ts">
  import { beforeNavigate, goto } from "$app/navigation";
  import { onMount } from "svelte";
  import "../app.css";
  import AppShell from "#lib/components/layout/AppShell.svelte";
  import BrandIcon from "#lib/components/BrandIcon.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
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
  import PlugZapIcon from "@lucide/svelte/icons/plug-zap";
  import {
    nodeSettingsPath,
    resolveDefaultNodeId,
    settingsHomePath,
  } from "#lib/settings-routes.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";

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
        // The same shortcut opens whichever navigation surface this shell has.
        if (uiTraits.overlayNav) layoutState.toggleMobileDrawer();
        else layoutState.toggleSidebar();
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
  <AppShell>
    {@render children()}
  </AppShell>
{/if}
