<script lang="ts">
  import { cn } from "#lib/utils.ts";

  let {
    diff,
    class: className = "",
    maxHeight = "16rem",
  }: { diff: string; class?: string; maxHeight?: string } = $props();

  const lines = $derived(diff.split("\n"));

  function lineClass(line: string): string {
    if (line.startsWith("+++") || line.startsWith("---")) {
      return "text-muted-foreground";
    }
    if (line.startsWith("@@")) return "text-sky-500";
    if (line.startsWith("+")) return "bg-ok-soft/50 text-ok";
    if (line.startsWith("-")) return "bg-danger-soft/50 text-danger";
    return "";
  }
</script>

<pre
  class={cn(
    "bg-muted/30 overflow-auto rounded-md border p-2 font-mono text-[13px] leading-relaxed",
    className,
  )}
  style={`max-height:${maxHeight}`}>{#each lines as line}<span class={lineClass(line)}
      >{line}{"\n"}</span
    >{/each}</pre>
