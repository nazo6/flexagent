<script lang="ts">
  import { page } from "$app/state";
  import BrandIcon from "#lib/components/BrandIcon.svelte";
  import ConnectionMenu from "#lib/components/ConnectionMenu.svelte";
  import ProblemBadge from "#lib/components/ProblemBadge.svelte";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import SearchIcon from "@lucide/svelte/icons/search";

  let { attentionCount }: { attentionCount: number } = $props();

  let brand = $state<ReturnType<typeof BrandIcon> | null>(null);

  /**
   * Replay the brand reveal on a plain primary click. Modifier clicks open the
   * link elsewhere, so they leave the logo as a static mark.
   */
  function handleBrandClick(event: MouseEvent): void {
    if (
      event.button === 0 &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.shiftKey &&
      !event.altKey
    ) {
      brand?.replay();
    }
  }
</script>

<header
  class="wco-titlebar wco-titlebar-bar bg-background/85 flex shrink-0 items-center justify-between border-b border-border px-3 py-2 backdrop-blur"
>
  <button
    type="button"
    class="hover:bg-accent relative flex size-8 items-center justify-center rounded-md border text-muted-foreground"
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
    <a href="/" class="flex items-center gap-1.5 font-semibold text-sm" onclick={handleBrandClick}>
      <BrandIcon bind:this={brand} class="size-6" />
      <span>FlexAgent</span>
    </a>
    <ConnectionMenu variant="scope-compact" align="end" />
  </div>

  <div class="flex items-center gap-1">
    <ProblemBadge class="size-8 px-0" />
    {#if page.url.pathname === "/"}
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
