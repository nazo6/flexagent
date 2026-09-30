<script lang="ts">
  import DiffViewer from './DiffViewer.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { cn } from '$lib/utils';
  import type { FileDiff } from '$lib/generated/FileDiff';
  import type { WorkspaceDiffResponse } from '$lib/generated/WorkspaceDiffResponse';
  import { sync } from '$lib/stores/app.svelte';
  import type { TimelineItem } from '$lib/sync/reducer';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  let { sessionId, items }: { sessionId: string; items: TimelineItem[] } = $props();

  type Scope = 'session' | 'branch_base' | 'uncommitted';

  let scope = $state<Scope>('session');
  let worktreeDiff = $state<WorkspaceDiffResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selectedPath = $state<string | null>(null);

  /** 「セッションの変更」: `tool_call` の Diff をパス単位で集約する (後勝ち)。 */
  const sessionFiles = $derived.by(() => {
    const byPath = new Map<string, FileDiff>();
    for (const item of items) {
      if (item.kind === 'tool' && item.diff !== null) {
        byPath.set(item.diff.path, item.diff);
      }
    }
    return [...byPath.values()];
  });

  const files = $derived(scope === 'session' ? sessionFiles : (worktreeDiff?.files ?? []));
  const selected = $derived(
    files.find((file) => file.path === selectedPath) ?? files.at(0) ?? null
  );

  async function loadWorktreeDiff(): Promise<void> {
    if (scope === 'session') return;
    loading = true;
    error = null;
    try {
      worktreeDiff = await sync.connection.client.sessionDiff(sessionId, scope);
      selectedPath = null;
    } catch (err) {
      worktreeDiff = null;
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    const current = scope;
    if (current === 'session') return;
    void loadWorktreeDiff();
  });

  const scopeOptions: { value: Scope; label: string }[] = [
    { value: 'session', label: 'セッションの変更' },
    { value: 'branch_base', label: 'Worktree: vs Base' },
    { value: 'uncommitted', label: 'Worktree: vs HEAD' }
  ];
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center gap-1.5">
    {#each scopeOptions as option (option.value)}
      <Button
        size="sm"
        variant={scope === option.value ? 'default' : 'outline'}
        onclick={() => (scope = option.value)}
      >
        {option.label}
      </Button>
    {/each}
    {#if scope !== 'session'}
      <Button
        size="sm"
        variant="ghost"
        disabled={loading}
        onclick={() => void loadWorktreeDiff()}
      >
        <RefreshCwIcon />
        再取得
      </Button>
    {/if}
    {#if worktreeDiff !== null && scope !== 'session'}
      <span class="text-muted-foreground ml-auto text-xs">
        {worktreeDiff.base_branch ?? worktreeDiff.scope}
        {#if worktreeDiff.head_commit !== ''}@ {worktreeDiff.head_commit.slice(0, 8)}{/if}
      </span>
    {/if}
  </div>

  {#if error !== null}
    <p class="text-destructive text-sm">{error}</p>
  {/if}

  {#if scope === 'session' && sessionFiles.length === 0}
    <p class="text-muted-foreground text-sm">
      このセッションでエージェントが編集したファイルはまだありません。
    </p>
  {:else if scope !== 'session' && loading}
    <p class="text-muted-foreground text-sm">Diff を取得しています…</p>
  {:else if files.length === 0 && error === null}
    <p class="text-muted-foreground text-sm">変更はありません。</p>
  {:else if files.length > 0}
    <div class="grid gap-3 lg:grid-cols-[minmax(0,20rem)_minmax(0,1fr)]">
      <ul class="flex max-h-[65svh] list-none flex-col gap-0.5 overflow-y-auto p-0">
        {#each files as file (file.path)}
          <li>
            <button
              type="button"
              class={cn(
                'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs',
                selected?.path === file.path ? 'bg-accent text-accent-foreground' : 'hover:bg-muted'
              )}
              onclick={() => (selectedPath = file.path)}
            >
              <span class="truncate font-mono">{file.path}</span>
              <span class="ml-auto flex shrink-0 items-center gap-1">
                {#if file.additions > 0}
                  <Badge variant="secondary" class="text-emerald-600 dark:text-emerald-400">
                    +{file.additions}
                  </Badge>
                {/if}
                {#if file.deletions > 0}
                  <Badge variant="secondary" class="text-red-600 dark:text-red-400">
                    -{file.deletions}
                  </Badge>
                {/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
      <div class="min-w-0">
        <DiffViewer file={selected} />
      </div>
    </div>
  {/if}
</div>
