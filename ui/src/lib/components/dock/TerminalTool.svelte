<script lang="ts">
  import TerminalView from "#lib/components/terminal/TerminalView.svelte";
  import type { DockContext } from "#lib/stores/dock.svelte.ts";

  let { ctx, active }: { ctx: DockContext; active: boolean } = $props();
</script>

<div class="flex min-h-0 flex-1 flex-col overflow-hidden p-2">
  <TerminalView sessionId={ctx.sessionId} nodeId={ctx.nodeId} path={ctx.localPath} fill {active} />
</div>
