<script lang="ts">
  import { cn } from '$lib/utils';

  let {
    diff,
    class: className = '',
    maxHeight = '16rem'
  }: { diff: string; class?: string; maxHeight?: string } = $props();

  const lines = $derived(diff.split('\n'));

  function lineClass(line: string): string {
    if (line.startsWith('+++') || line.startsWith('---')) {
      return 'text-muted-foreground';
    }
    if (line.startsWith('@@')) return 'text-sky-500';
    if (line.startsWith('+')) return 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400';
    if (line.startsWith('-')) return 'bg-red-500/10 text-red-600 dark:text-red-400';
    return '';
  }
</script>

<pre
  class={cn(
    'bg-muted/30 overflow-auto rounded-md border p-2 font-mono text-xs leading-relaxed',
    className
  )}
  style={`max-height:${maxHeight}`}
>{#each lines as line}<span class={lineClass(line)}>{line}{'\n'}</span>{/each}</pre>
