<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import type { FsEntry } from '$lib/generated/FsEntry';
  import { sync } from '$lib/stores/app.svelte';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';
  import EyeIcon from '@lucide/svelte/icons/eye';
  import EyeOffIcon from '@lucide/svelte/icons/eye-off';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import FolderOpenIcon from '@lucide/svelte/icons/folder-open';
  import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
  import Loader2Icon from '@lucide/svelte/icons/loader-2';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  interface Props {
    open: boolean;
    nodeId: string;
    initialPath?: string;
    title?: string;
    onSelect: (selectedPath: string) => void;
  }

  let {
    open = $bindable(false),
    nodeId,
    initialPath = '',
    title = 'フォルダを選択',
    onSelect
  }: Props = $props();

  let currentPath = $state('');
  let parentPath = $state<string | null>(null);
  let entries = $state<FsEntry[]>([]);
  let selectedPath = $state('');
  let inputPath = $state('');
  let loading = $state(false);
  let error = $state<string | null>(null);
  let showHidden = $state(false);

  // ダイアログが開いたときに初期パスまたはホームを読み込む
  $effect(() => {
    if (open && nodeId) {
      void loadDirectory(initialPath || undefined);
    }
  });

  async function loadDirectory(path?: string) {
    if (!nodeId) return;
    loading = true;
    error = null;
    try {
      const res = await sync.connection.client.browseFs(nodeId, path);
      currentPath = res.current_path;
      parentPath = res.parent_path;
      entries = res.entries;
      inputPath = res.current_path;
      selectedPath = res.current_path;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      loading = false;
    }
  }

  const displayedEntries = $derived.by(() => {
    return entries.filter((e) => {
      // フォルダブラウザなのでディレクトリのみを表示
      if (!e.is_dir) return false;
      if (!showHidden && e.is_hidden) return false;
      return true;
    });
  });

  function handleSelectEntry(entry: FsEntry) {
    selectedPath = entry.path;
  }

  function handleOpenEntry(entry: FsEntry) {
    void loadDirectory(entry.path);
  }

  function handleGoUp() {
    if (parentPath !== null) {
      void loadDirectory(parentPath);
    }
  }

  function handleGoDrives() {
    void loadDirectory('');
  }

  function handlePathSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (inputPath.trim()) {
      void loadDirectory(inputPath.trim());
    }
  }

  function handleConfirm() {
    const target = selectedPath || currentPath;
    if (target) {
      onSelect(target);
      open = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-xl max-h-[85vh] flex flex-col p-4 gap-3">
    <Dialog.Header class="gap-1">
      <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
        <FolderOpenIcon class="size-4 text-primary" />
        <span>{title}</span>
      </Dialog.Title>
      <Dialog.Description class="text-xs text-muted-foreground">
        セッションを実行するワーキングディレクトリを選択してください。
      </Dialog.Description>
    </Dialog.Header>

    <!-- パス操作ツールバー -->
    <div class="flex items-center gap-1.5">
      <Button
        variant="outline"
        size="icon"
        class="size-8 shrink-0"
        disabled={parentPath === null || loading}
        onclick={handleGoUp}
        title="上の階層へ移動"
      >
        <ArrowUpIcon class="size-3.5" />
      </Button>

      <Button
        variant="outline"
        size="icon"
        class="size-8 shrink-0"
        disabled={loading}
        onclick={handleGoDrives}
        title="ドライブ一覧 / ルートへ移動"
      >
        <HardDriveIcon class="size-3.5" />
      </Button>

      <form onsubmit={handlePathSubmit} class="flex-1 flex items-center gap-1">
        <Input
          bind:value={inputPath}
          placeholder="パスを入力して Enter (例: C:\repos or /home/user)"
          class="h-8 text-xs font-mono"
          disabled={loading}
        />
        <Button
          type="submit"
          variant="secondary"
          size="sm"
          class="h-8 px-2 text-xs shrink-0"
          disabled={loading || !inputPath.trim()}
          title="指定したパスへ移動"
        >
          <CornerDownLeftIcon class="size-3" />
        </Button>
      </form>

      <Button
        variant="ghost"
        size="icon"
        class="size-8 shrink-0 text-muted-foreground hover:text-foreground"
        disabled={loading}
        onclick={() => loadDirectory(currentPath || undefined)}
        title="再読み込み"
      >
        <RefreshCwIcon class="size-3.5 {loading ? 'animate-spin' : ''}" />
      </Button>
    </div>

    <!-- フォルダ一覧領域 -->
    <div class="flex-1 min-h-60 max-h-72 overflow-y-auto rounded-md border bg-muted/20 p-1 flex flex-col">
      {#if loading}
        <div class="flex flex-1 items-center justify-center py-12 text-muted-foreground gap-2 text-xs">
          <Loader2Icon class="size-4 animate-spin text-primary" />
          <span>フォルダを読み込み中…</span>
        </div>
      {:else if error}
        <div class="flex flex-1 flex-col items-center justify-center p-4 text-center gap-2">
          <p class="text-xs text-destructive">{error}</p>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            onclick={() => loadDirectory(parentPath ?? '')}
          >
            戻る
          </Button>
        </div>
      {:else if displayedEntries.length === 0}
        <div class="flex flex-1 items-center justify-center py-12 text-muted-foreground text-xs">
          サブフォルダはありません
        </div>
      {:else}
        <ul class="flex list-none flex-col gap-0.5 p-0 m-0">
          {#each displayedEntries as entry (entry.path)}
            {@const isSelected = selectedPath === entry.path}
            <li>
              <div
                role="button"
                tabindex="0"
                class="group flex items-center justify-between rounded px-2 py-1.5 text-xs transition-colors cursor-pointer select-none {isSelected
                  ? 'bg-primary/15 text-primary font-medium'
                  : 'hover:bg-muted text-foreground/90'}"
                onclick={() => handleSelectEntry(entry)}
                ondblclick={() => handleOpenEntry(entry)}
                onkeydown={(e) => {
                  if (e.key === 'Enter') handleOpenEntry(entry);
                }}
              >
                <div class="flex min-w-0 flex-1 items-center gap-2">
                  <FolderIcon class="size-3.5 shrink-0 text-amber-500/80" />
                  <span class="truncate font-mono text-xs">{entry.name}</span>
                  {#if entry.is_hidden}
                    <span class="text-[10px] text-muted-foreground opacity-60">(隠し)</span>
                  {/if}
                </div>

                <button
                  type="button"
                  class="opacity-0 group-hover:opacity-100 hover:bg-background/80 text-muted-foreground hover:text-foreground rounded p-1 transition-opacity"
                  title="このフォルダの中に入る"
                  onclick={(e) => {
                    e.stopPropagation();
                    handleOpenEntry(entry);
                  }}
                >
                  <ChevronRightIcon class="size-3.5" />
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <!-- 下部オプション & 現在の選択 -->
    <div class="flex items-center justify-between gap-2 text-xs pt-1">
      <button
        type="button"
        class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1.5 transition-colors cursor-pointer"
        onclick={() => (showHidden = !showHidden)}
      >
        {#if showHidden}
          <EyeIcon class="size-3.5" />
          <span>隠しフォルダを表示中</span>
        {:else}
          <EyeOffIcon class="size-3.5" />
          <span>隠しフォルダを表示</span>
        {/if}
      </button>

      <div class="flex min-w-0 max-w-[65%] items-center gap-1 text-[11px] text-muted-foreground font-mono truncate">
        <span class="shrink-0">選択中:</span>
        <span class="truncate text-foreground font-medium" title={selectedPath || currentPath || '(未選択)'}>
          {selectedPath || currentPath || '(未選択)'}
        </span>
      </div>
    </div>

    <!-- ダイアログフッター -->
    <Dialog.Footer class="gap-2 sm:gap-2">
      <Button
        variant="outline"
        size="sm"
        class="text-xs h-8"
        onclick={() => (open = false)}
      >
        キャンセル
      </Button>
      <Button
        size="sm"
        class="text-xs h-8 gap-1.5"
        disabled={loading || (!selectedPath && !currentPath)}
        onclick={handleConfirm}
      >
        <FolderOpenIcon class="size-3.5" />
        <span>このフォルダを選択</span>
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
