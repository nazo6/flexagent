<script lang="ts">
  import { m } from "#lib/paraglide/messages.js";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";

  interface Props {
    parentSessionId: string;
    title?: string | null;
  }

  let { parentSessionId, title = null }: Props = $props();

  const label = $derived(title ?? parentSessionId.slice(0, 8));
</script>

<div class="text-muted-foreground flex items-center gap-2 py-1 text-[11px]">
  <span class="bg-border/60 h-px flex-1"></span>
  <a
    href={`/sessions/${parentSessionId}`}
    class="border-border bg-card hover:bg-accent hover:text-foreground inline-flex max-w-[80%] items-center gap-1.5 rounded-full border px-2.5 py-1 shadow-sm transition-colors"
    title={m.session_fork_origin({ title: label })}
  >
    <GitForkIcon class="size-3 shrink-0" />
    <span class="truncate">{m.session_fork_origin({ title: label })}</span>
  </a>
  <span class="bg-border/60 h-px flex-1"></span>
</div>
