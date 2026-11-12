<script lang="ts">
  import { page } from "$app/state";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";

  const notFound = $derived(page.status === 404);
  const title = $derived(
    notFound ? m.error_page_not_found_title() : m.error_page_unexpected_title(),
  );
  const body = $derived(notFound ? m.error_page_not_found_body() : m.error_page_unexpected_body());

  function reload(): void {
    location.reload();
  }

  function goBack(): void {
    history.back();
  }
</script>

<svelte:head>
  <title>{title} - FlexAgent</title>
</svelte:head>

<div class="flex min-h-full items-center justify-center p-6">
  <div class="flex max-w-md flex-col items-center gap-3 text-center">
    <TriangleAlertIcon class="text-warn size-8" />
    <h1 class="text-lg font-semibold">{title}</h1>
    <p class="text-muted-foreground text-sm">{body}</p>
    <div class="mt-1 flex items-center gap-2">
      <Button onclick={reload}>{m.error_page_reload()}</Button>
      <Button variant="outline" onclick={goBack}>{m.error_page_go_back()}</Button>
    </div>
    <details class="mt-2 w-full text-left">
      <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px]">
        {m.error_page_details()}
      </summary>
      <div class="text-muted-foreground mt-1 flex flex-col gap-1 text-[11px]">
        <p class="font-mono">status {page.status}</p>
        {#if page.error?.message}
          <p class="font-mono break-words">{page.error.message}</p>
        {/if}
      </div>
    </details>
  </div>
</div>
