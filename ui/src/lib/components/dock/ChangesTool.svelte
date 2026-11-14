<script lang="ts">
  import DiffPane from "#lib/components/diff/DiffPane.svelte";
  import type { DockContext } from "#lib/stores/dock.svelte.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";

  let { ctx, items }: { ctx: DockContext; items: TimelineItem[] } = $props();
</script>

<div class="app-scroll min-h-0 flex-1 overflow-y-auto p-3">
  <DiffPane sessionId={ctx.sessionId} {items} />
</div>
