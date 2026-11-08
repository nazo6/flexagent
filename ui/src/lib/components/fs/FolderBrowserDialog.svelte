<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import type { FsEntry } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import CornerDownLeftIcon from "@lucide/svelte/icons/corner-down-left";
  import EyeIcon from "@lucide/svelte/icons/eye";
  import EyeOffIcon from "@lucide/svelte/icons/eye-off";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import Loader2Icon from "@lucide/svelte/icons/loader-2";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  interface Props {
    open: boolean;
    nodeId: string;
    initialPath?: string;
    title?: string;
    /** Also show files and allow selecting them (default: folders only) */
    files?: boolean;
    onSelect: (selectedPath: string) => void;
  }

  let {
    open = $bindable(false),
    nodeId,
    initialPath = "",
    title = m.fs_select_folder(),
    files = false,
    onSelect,
  }: Props = $props();

  let currentPath = $state("");
  let parentPath = $state<string | null>(null);
  let entries = $state<FsEntry[]>([]);
  let selectedPath = $state("");
  let inputPath = $state("");
  let loading = $state(false);
  let error = $state<string | null>(null);
  let showHidden = $state(false);

  // Load the initial path (or home) when the dialog opens
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
      if (!files && !e.is_dir) return false;
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
    void loadDirectory("");
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
        {m.fs_description()}
      </Dialog.Description>
    </Dialog.Header>

    <!-- Path toolbar -->
    <div class="flex items-center gap-1.5">
      <Button
        variant="outline"
        size="icon"
        class="size-8 shrink-0"
        disabled={parentPath === null || loading}
        onclick={handleGoUp}
        title={m.fs_go_up()}
      >
        <ArrowUpIcon class="size-3.5" />
      </Button>

      <Button
        variant="outline"
        size="icon"
        class="size-8 shrink-0"
        disabled={loading}
        onclick={handleGoDrives}
        title={m.fs_go_root()}
      >
        <HardDriveIcon class="size-3.5" />
      </Button>

      <form onsubmit={handlePathSubmit} class="flex-1 flex items-center gap-1">
        <Input
          bind:value={inputPath}
          placeholder={m.fs_path_placeholder()}
          class="h-8 text-xs font-mono"
          disabled={loading}
        />
        <Button
          type="submit"
          variant="secondary"
          size="sm"
          class="h-8 px-2 text-xs shrink-0"
          disabled={loading || !inputPath.trim()}
          title={m.fs_go_to_path()}
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
        title={m.common_refresh()}
      >
        <RefreshCwIcon class="size-3.5 {loading ? 'animate-spin' : ''}" />
      </Button>
    </div>

    <!-- Folder list area -->
    <div
      class="flex-1 min-h-60 max-h-72 overflow-y-auto rounded-md border bg-muted/20 p-1 flex flex-col"
    >
      {#if loading}
        <div
          class="flex flex-1 items-center justify-center py-12 text-muted-foreground gap-2 text-xs"
        >
          <Loader2Icon class="size-4 animate-spin text-primary" />
          <span>{m.fs_loading_folders()}</span>
        </div>
      {:else if error}
        <div class="flex flex-1 flex-col items-center justify-center p-4 text-center gap-2">
          <p class="text-xs text-destructive">{error}</p>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            onclick={() => loadDirectory(parentPath ?? "")}
          >
            {m.fs_back()}
          </Button>
        </div>
      {:else if displayedEntries.length === 0}
        <div class="flex flex-1 items-center justify-center py-12 text-muted-foreground text-xs">
          {m.fs_no_subfolders()}
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
                ondblclick={() =>
                  entry.is_dir ? handleOpenEntry(entry) : handleSelectEntry(entry)}
                onkeydown={(e) => {
                  if (e.key === "Enter") handleOpenEntry(entry);
                }}
              >
                <div class="flex min-w-0 flex-1 items-center gap-2">
                  <FolderIcon class="size-3.5 shrink-0 text-amber-500/80" />
                  <span class="truncate font-mono text-xs">{entry.name}</span>
                  {#if entry.is_hidden}
                    <span class="text-[10px] text-muted-foreground opacity-60">
                      {m.fs_hidden()}
                    </span>
                  {/if}
                </div>

                <button
                  type="button"
                  class="opacity-0 group-hover:opacity-100 hover:bg-background/80 text-muted-foreground hover:text-foreground rounded p-1 transition-opacity"
                  title={m.fs_open_folder()}
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

    <!-- Bottom options and current selection -->
    <div class="flex items-center justify-between gap-2 text-xs pt-1">
      <button
        type="button"
        class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1.5 transition-colors cursor-pointer"
        onclick={() => (showHidden = !showHidden)}
      >
        {#if showHidden}
          <EyeIcon class="size-3.5" />
          <span>{m.fs_hidden_shown()}</span>
        {:else}
          <EyeOffIcon class="size-3.5" />
          <span>{m.fs_show_hidden()}</span>
        {/if}
      </button>

      <div
        class="flex min-w-0 max-w-[65%] items-center gap-1 text-[11px] text-muted-foreground font-mono truncate"
      >
        <span class="shrink-0">{m.fs_selected()}</span>
        <span
          class="truncate text-foreground font-medium"
          title={selectedPath || currentPath || m.fs_none_selected()}
        >
          {selectedPath || currentPath || m.fs_none_selected()}
        </span>
      </div>
    </div>

    <!-- Dialog footer -->
    <Dialog.Footer class="gap-2 sm:gap-2">
      <Button variant="outline" size="sm" class="text-xs h-8" onclick={() => (open = false)}>
        {m.common_cancel()}
      </Button>
      <Button
        size="sm"
        class="text-xs h-8 gap-1.5"
        disabled={loading || (!selectedPath && !currentPath)}
        onclick={handleConfirm}
      >
        <FolderOpenIcon class="size-3.5" />
        <span>{m.fs_select_folder_confirm()}</span>
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
