<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import "../app.css";
  import AppSidebar from "#lib/components/layout/AppSidebar.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ErrorBanner from "#lib/components/ErrorBanner.svelte";
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
  import PlugZapIcon from "@lucide/svelte/icons/plug-zap";
  import PlusIcon from "@lucide/svelte/icons/plus";

  let { children } = $props();

  let mobileDrawerOpen = $state(false);

  function onUnhandledRejection(event: PromiseRejectionEvent): void {
    problems.report("unhandled error", event.reason);
  }

  function onUncaughtError(event: ErrorEvent): void {
    problems.report("uncaught error", event.error ?? event.message);
  }

  onMount(() => {
    theme.init();
    window.addEventListener("unhandledrejection", onUnhandledRejection);
    window.addEventListener("error", onUncaughtError);
    void initApp();
    return () => {
      window.removeEventListener("unhandledrejection", onUnhandledRejection);
      window.removeEventListener("error", onUncaughtError);
    };
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
</script>

<ModeWatcher />
<Toaster richColors position="top-center" />
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
        <code>http://localhost:7860</code>{m.shell_server_down_hint_suffix()}
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
    <!-- Desktop sidebar (always visible on md and up) -->
    <div class="hidden md:flex md:w-[264px] lg:w-[288px] md:shrink-0 md:h-full">
      <AppSidebar />
    </div>

    <!-- Mobile drawer (Sheet) -->
    <Sheet.Root bind:open={mobileDrawerOpen}>
      <Sheet.Content side="left" class="p-0 w-80 max-w-[85vw]">
        <AppSidebar onNavigate={() => (mobileDrawerOpen = false)} />
      </Sheet.Content>
    </Sheet.Root>

    <!-- Main area -->
    <div class="flex flex-1 flex-col h-full overflow-hidden">
      <!-- Mobile top bar (visible below md) -->
      <header
        class="bg-background/85 flex items-center justify-between border-b border-border px-3 py-2 backdrop-blur md:hidden shrink-0"
      >
        <button
          type="button"
          class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
          onclick={() => (mobileDrawerOpen = true)}
          aria-label={m.shell_open_menu()}
        >
          <MenuIcon class="size-4" />
        </button>

        <a href="/" class="flex items-center gap-1.5 font-semibold text-sm">
          <div
            class="bg-primary text-primary-foreground flex size-6 items-center justify-center rounded-md font-mono text-[11px] font-bold"
          >
            fxg
          </div>
          <span>FlexAgent</span>
        </a>

        <a
          href="/"
          class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
          aria-label={m.shell_new_session()}
        >
          <PlusIcon class="size-4" />
        </a>
      </header>

      <ErrorBanner onaction={handleProblemAction} />

      <main class="relative flex-1 overflow-y-auto">
        {@render children()}
      </main>
    </div>
  </div>
{/if}
