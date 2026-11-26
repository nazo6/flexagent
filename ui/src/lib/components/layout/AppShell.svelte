<script lang="ts">
  import { page } from "$app/state";
  import type { Snippet } from "svelte";
  import AppSidebar from "./AppSidebar.svelte";
  import CompactTopBar from "./CompactTopBar.svelte";
  import MobileDrawer from "./MobileDrawer.svelte";
  import SidebarReopen from "./SidebarReopen.svelte";
  import { sessionNeedsAttention } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import { uiTraits } from "#lib/ui-traits.svelte.ts";

  let { children }: { children: Snippet } = $props();

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
  const drawerOpen = $derived(uiTraits.overlayNav && layoutState.mobileDrawerOpen);

  // The drawer is only meaningful in the overlay shell. Drop the flag when the
  // window grows so shrinking back does not reopen a stale drawer.
  $effect(() => {
    if (!uiTraits.overlayNav && layoutState.mobileDrawerOpen) {
      layoutState.setMobileDrawer(false);
    }
  });
</script>

<!-- The page subtree stays inside this single `<main>` while the chrome
     around it switches, so a resize never remounts the current route. -->
<div
  class="safe-area-shell app-viewport bg-background text-foreground flex w-full overflow-hidden"
  class:wco-sidebar-collapsed={!uiTraits.overlayNav && layoutState.sidebarCollapsed}
>
  {#if !uiTraits.overlayNav && !layoutState.sidebarCollapsed}
    <div class="flex h-full w-[264px] shrink-0 xl:w-[288px]">
      <AppSidebar />
    </div>
  {/if}

  <div class="relative flex h-full flex-1 flex-col overflow-hidden" inert={drawerOpen}>
    {#if uiTraits.overlayNav}
      {#if !isSessionRoute}
        <CompactTopBar {attentionCount} />
      {/if}
    {:else}
      {#if layoutState.sidebarCollapsed && !isSessionRoute}
        <SidebarReopen {attentionCount} />
      {/if}
      {#if !isSessionRoute}
        <!-- Window Controls Overlay strip. Session pages use their own header
             as the title bar, so this only fills the strip for the shell routes
             and keeps the native controls off the content. -->
        <div
          class="wco-titlebar wco-main-titlebar hidden shrink-0 border-b border-border bg-background/85 backdrop-blur"
          aria-hidden="true"
        ></div>
      {/if}
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
