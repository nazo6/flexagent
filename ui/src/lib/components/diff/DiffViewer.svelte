<script lang="ts">
  import type { FileDiff } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import MonacoDiff from "./MonacoDiff.svelte";
  import ShikiDiff from "./ShikiDiff.svelte";

  let { file }: { file: FileDiff | null } = $props();

  /** Desktop (>= 768px) uses the Monaco diff editor; mobile uses a shiki unified diff. */
  let viewportWidth = $state(typeof window === "undefined" ? 0 : window.innerWidth);

  function updateViewportWidth() {
    viewportWidth = window.innerWidth;
  }

  $effect(() => {
    if (typeof window === "undefined") return;
    window.addEventListener("resize", updateViewportWidth);
    return () => window.removeEventListener("resize", updateViewportWidth);
  });

  const canUseMonaco = $derived(
    viewportWidth >= 768 && file !== null && file.old_text !== null && file.new_text !== null,
  );
</script>

{#if file === null}
  <p class="text-muted-foreground flex h-full items-center justify-center text-sm">
    {m.diff_select_file()}
  </p>
{:else}
  <div class="flex flex-col gap-2">
    <div class="flex flex-wrap items-center gap-2 text-[13px]">
      <span class="font-mono break-all">{file.path}</span>
      <span class="text-ok">+{file.additions}</span>
      <span class="text-danger">-{file.deletions}</span>
    </div>
    {#if canUseMonaco && file !== null}
      <MonacoDiff {file} />
    {:else if file.unified_diff !== ""}
      <ShikiDiff diff={file.unified_diff} />
    {:else}
      <p class="text-muted-foreground text-sm">
        {m.diff_binary_or_unsupported()}
      </p>
    {/if}
  </div>
{/if}
