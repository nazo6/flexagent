<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import "../app.css";
  import AppSidebar from "#lib/components/layout/AppSidebar.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import ProblemCenter from "#lib/components/ProblemCenter.svelte";
  import TokenDialog from "#lib/components/TokenDialog.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import * as Sheet from "#lib/components/ui/sheet/index.ts";
  import { Toaster } from "#lib/components/ui/sonner/index.ts";
  import { connection, initApp, sync } from "#lib/stores/app.svelte.ts";
  import { problems, type AppProblem } from "#lib/stores/problems.svelte.ts";
  import { theme } from "#lib/theme.svelte.ts";
  import type { RecoveryAction } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { ModeWatcher } from "mode-watcher";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PlugZapIcon from "@lucide/svelte/icons/plug-zap";
  import PlusIcon from "@lucide/svelte/icons/plus";

  import { page } from "$app/state";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import InboxIcon from "@lucide/svelte/icons/inbox";
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
    void initApp();
  });

  /**
   * Executes a server-provided recovery action.
   *
   * Navigation-based actions open the screen where the fix is made; execution
   * of the fix itself stays in that screen (Phase 5 will add bespoke handlers).
   */
  function handleProblemAction(problem: AppProblem, action: RecoveryAction): void {
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
      case "open_config":
      case "select_online_node":
      case "recreate_worktree":
      case "export_diagnostics":
        void goto("/settings");
        break;
      case "login_agent":
        void goto("/settings?tab=agents");
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
</script>

<ModeWatcher />
<svelte:window
  onerror={onUncaughtError}
  onunhandledrejection={onUnhandledRejection}
  onkeydown={onGlobalKeydown}
/>
<Toaster richColors position="top-center" />
<ProblemCenter onaction={handleProblemAction} />
<TokenDialog />

{#if !connection.ready}
  <div class="bg-background text-muted-foreground flex min-h-svh items-center justify-center gap-2">
    <LoaderCircleIcon class="size-4 animate-spin" />
    {m.shell_checking_connection()}
  </div>
{:else if connection.error !== null}
  <div class="bg-background flex min-h-svh items-center justify-center p-6">
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
{:else if connection.tokenRequired}
  <div class="bg-background min-h-svh"></div>
{:else}
  <div class="bg-background text-foreground flex h-svh w-full overflow-hidden">
    <!-- Desktop sidebar (persistent on lg and up) -->
    {#if !layoutState.sidebarCollapsed}
      <div class="hidden lg:flex lg:w-[264px] xl:w-[288px] lg:shrink-0 lg:h-full">
        <AppSidebar />
      </div>
    {:else}
      <!-- Collapsed rail: keeps a visible way to reopen the sidebar on desktop. -->
      <div class="hidden w-10 shrink-0 flex-col items-center border-r border-border py-2 lg:flex">
        <button
          type="button"
          class="hover:bg-accent text-muted-foreground hover:text-foreground flex size-7 items-center justify-center rounded-md border border-border"
          onclick={() => layoutState.setSidebarCollapsed(false)}
          title={m.shell_show_sidebar()}
          aria-label={m.shell_show_sidebar()}
        >
          <PanelLeftOpenIcon class="size-4" />
        </button>
        <ProblemBadge class="mt-1 size-7 px-0" />
      </div>
    {/if}

    <!-- Mobile drawer (Sheet) -->
    <Sheet.Root bind:open={layoutState.mobileDrawerOpen}>
      <Sheet.Content side="left" class="p-0 w-80 max-w-[85vw]">
        <AppSidebar onNavigate={() => (layoutState.mobileDrawerOpen = false)} />
      </Sheet.Content>
    </Sheet.Root>

    <!-- Main area -->
    <div class="flex flex-1 flex-col h-full overflow-hidden">
      <!-- Mobile top bar (visible below lg; session pages own their header) -->
      {#if !isSessionRoute}
        <header
          class="bg-background/85 flex items-center justify-between border-b border-border px-3 py-2 backdrop-blur lg:hidden shrink-0"
        >
          <button
            type="button"
            class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
            onclick={() => layoutState.setMobileDrawer(true)}
            aria-label={m.shell_open_menu()}
          >
            <MenuIcon class="size-4" />
          </button>

          <div class="flex min-w-0 items-center gap-2">
            <a href="/" class="flex items-center gap-1.5 font-semibold text-sm">
              <div
                class="bg-primary text-primary-foreground flex size-6 items-center justify-center rounded-md font-mono text-[11px] font-bold"
              >
                fxg
              </div>
              <span>FlexAgent</span>
            </a>
            <ConnectionMenu variant="scope-compact" align="end" />
          </div>

          <div class="flex items-center gap-1">
            <ProblemBadge class="size-8 px-0" />
            {#if sync.inbox.length > 0}
              <a
                href="/inbox"
                class="hover:bg-accent relative flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                title={m.shell_approval_inbox()}
                aria-label={m.shell_approval_inbox()}
              >
                <InboxIcon class="size-4" />
                <span
                  class="bg-destructive text-destructive-foreground absolute -top-1 -right-1 flex size-4 items-center justify-center rounded-full text-[10px] font-bold leading-none"
                >
                  {sync.inbox.length}
                </span>
              </a>
            {:else if page.url.pathname === "/"}
              <a
                href="/search"
                class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                title={m.shell_full_text_search()}
                aria-label={m.shell_full_text_search()}
              >
                <SearchIcon class="size-4" />
              </a>
            {:else}
              <a
                href="/"
                class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
                title={m.shell_new_session()}
                aria-label={m.shell_new_session()}
              >
                <PlusIcon class="size-4" />
              </a>
            {/if}
          </div>
        </header>
      {/if}

      <main class="relative flex-1 overflow-y-auto" class:app-scroll={mainScrolls}>
        {@render children()}
      </main>
    </div>
  </div>
{/if}
