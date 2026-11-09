<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { buildBreadcrumbs, matchesFolderFilter, type QuickPath } from "#lib/folder-picker.ts";
  import type { FsEntry } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import CornerDownLeftIcon from "@lucide/svelte/icons/corner-down-left";
  import EyeIcon from "@lucide/svelte/icons/eye";
  import EyeOffIcon from "@lucide/svelte/icons/eye-off";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import HardDriveIcon from "@lucide/svelte/icons/hard-drive";
  import HomeIcon from "@lucide/svelte/icons/house";
  import Loader2Icon from "@lucide/svelte/icons/loader-2";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import type { Snippet } from "svelte";

  let {
    nodeId,
    active = true,
    mode = "select",
    initialPath = "",
    quickPaths = [],
    files = false,
    class: className = "",
    listClass = "h-64",
    onpathchange,
    onselectionchange,
    onloadingchange,
    onerror,
    children,
  }: {
    nodeId: string;
    /** Whether the picker is visible / should load. */
    active?: boolean;
    /** `select` = click selects, double-click opens; `navigate` = click opens. */
    mode?: "select" | "navigate";
    initialPath?: string;
    quickPaths?: QuickPath[];
    files?: boolean;
    class?: string;
    /** Fixed height for the scrollable entry list (keeps the dialog from resizing). */
    listClass?: string;
    onpathchange?: (path: string) => void;
    onselectionchange?: (path: string) => void;
    onloadingchange?: (loading: boolean) => void;
    onerror?: (error: string) => void;
    /** Optional content rendered above the folder list. */
    children?: Snippet;
  } = $props();

  let currentPath = $state("");
  let parentPath = $state<string | null>(null);
  let entries = $state<FsEntry[]>([]);
  let selectedPath = $state("");
  let inputPath = $state("");
  let filter = $state("");
  let loading = $state(false);
  let error = $state<string | null>(null);
  let showHidden = $state(false);

  $effect(() => {
    if (!active || nodeId === "") return;
    void loadDirectory(initialPath || undefined);
  });

  async function loadDirectory(path?: string) {
    if (!nodeId) return;
    loading = true;
    error = null;
    onloadingchange?.(true);
    try {
      const res = await sync.connection.client.browseFs(nodeId, path);
      currentPath = res.current_path;
      parentPath = res.parent_path;
      entries = res.entries;
      inputPath = res.current_path;
      filter = "";
      selectedPath = mode === "select" ? res.current_path : "";
      onselectionchange?.(selectedPath);
      onpathchange?.(res.current_path);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      selectedPath = "";
      onselectionchange?.("");
      onerror?.(message);
    } finally {
      loading = false;
      onloadingchange?.(false);
    }
  }

  const displayedEntries = $derived.by(() =>
    entries.filter((entry) => {
      if (!files && !entry.is_dir) return false;
      if (!showHidden && entry.is_hidden) return false;
      return matchesFolderFilter(entry.name, filter);
    }),
  );

  const breadcrumbs = $derived(buildBreadcrumbs(currentPath));

  function handleSelectEntry(entry: FsEntry): void {
    if (mode === "navigate" && entry.is_dir) {
      void loadDirectory(entry.path);
      return;
    }
    selectedPath = entry.path;
    onselectionchange?.(selectedPath);
  }

  function handleOpenEntry(entry: FsEntry): void {
    if (entry.is_dir) void loadDirectory(entry.path);
    else handleSelectEntry(entry);
  }

  function handleGoUp(): void {
    if (parentPath !== null) void loadDirectory(parentPath);
  }

  function handleGoHome(): void {
    void loadDirectory(undefined);
  }

  function handleGoDrives(): void {
    void loadDirectory("");
  }

  function handlePathSubmit(event: SubmitEvent): void {
    event.preventDefault();
    if (inputPath.trim()) void loadDirectory(inputPath.trim());
  }
</script>

<div class={cn("flex min-h-0 flex-col gap-2", className)}>
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
      onclick={handleGoHome}
      title={m.fs_go_home()}
    >
      <HomeIcon class="size-3.5" />
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
    <form onsubmit={handlePathSubmit} class="flex min-w-0 flex-1 items-center gap-1">
      <Input
        bind:value={inputPath}
        placeholder={m.fs_path_placeholder()}
        class="h-8 min-w-0 flex-1 font-mono text-xs"
        disabled={loading}
      />
      <Button
        type="submit"
        variant="secondary"
        size="sm"
        class="h-8 shrink-0 px-2 text-xs"
        disabled={loading || !inputPath.trim()}
        title={m.fs_go_to_path()}
      >
        <CornerDownLeftIcon class="size-3" />
      </Button>
    </form>
    <Button
      variant="ghost"
      size="icon"
      class="text-muted-foreground hover:text-foreground size-8 shrink-0"
      disabled={loading}
      onclick={() => void loadDirectory(currentPath || undefined)}
      title={m.common_refresh()}
    >
      <RefreshCwIcon class="size-3.5 {loading ? 'animate-spin' : ''}" />
    </Button>
  </div>

  {#if breadcrumbs.length > 0}
    <div
      class="flex flex-nowrap items-center gap-0.5 overflow-x-auto font-mono text-[11px] whitespace-nowrap"
    >
      {#each breadcrumbs as crumb, index (crumb.path)}
        {#if index > 0}
          <span class="text-muted-foreground/50">/</span>
        {/if}
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground max-w-40 truncate rounded px-1 py-0.5 hover:bg-muted"
          onclick={() => void loadDirectory(crumb.path)}
          title={crumb.path}
        >
          {crumb.label}
        </button>
      {/each}
    </div>
  {/if}

  {#if quickPaths.length > 0}
    <div class="flex flex-nowrap items-center gap-1 overflow-x-auto">
      <span class="text-muted-foreground shrink-0 text-[10px]">{m.fs_recent_folders()}</span>
      {#each quickPaths as quick (quick.path)}
        <button
          type="button"
          class="bg-muted/50 hover:bg-muted text-muted-foreground hover:text-foreground max-w-56 truncate rounded-full border px-2 py-0.5 text-[10px]"
          onclick={() => void loadDirectory(quick.path)}
          title={quick.description ?? quick.path}
        >
          {quick.label}
        </button>
      {/each}
    </div>
  {/if}

  {@render children?.()}

  <div class="flex items-center gap-1.5">
    <div
      class="border-input bg-background flex min-w-0 flex-1 items-center gap-1.5 rounded-md border px-2 py-1"
    >
      <SearchIcon class="text-muted-foreground size-3.5 shrink-0" />
      <input
        bind:value={filter}
        class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-xs outline-none"
        placeholder={m.fs_filter_folders()}
      />
    </div>
    <button
      type="button"
      class="text-muted-foreground hover:text-foreground inline-flex shrink-0 items-center gap-1 text-[10px]"
      onclick={() => (showHidden = !showHidden)}
      title={showHidden ? m.fs_hidden_shown() : m.fs_show_hidden()}
    >
      {#if showHidden}
        <EyeIcon class="size-3.5" />
      {:else}
        <EyeOffIcon class="size-3.5" />
      {/if}
    </button>
  </div>

  <div class={cn("bg-muted/20 flex flex-col overflow-y-auto rounded-md border p-1", listClass)}>
    {#if loading}
      <div
        class="text-muted-foreground flex flex-1 items-center justify-center gap-2 py-12 text-xs"
      >
        <Loader2Icon class="text-primary size-4 animate-spin" />
        <span>{m.fs_loading_folders()}</span>
      </div>
    {:else if error}
      <div class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
        <p class="text-destructive text-xs">{error}</p>
        <Button
          variant="outline"
          size="sm"
          class="h-7 text-xs"
          onclick={() => void loadDirectory(parentPath ?? undefined)}
        >
          {m.fs_back()}
        </Button>
      </div>
    {:else if displayedEntries.length === 0}
      <div class="text-muted-foreground flex flex-1 items-center justify-center py-12 text-xs">
        {filter.trim() === "" ? m.fs_no_subfolders() : m.fs_no_matching_folders()}
      </div>
    {:else}
      <ul class="m-0 flex list-none flex-col gap-0.5 p-0">
        {#each displayedEntries as entry (entry.path)}
          {@const isSelected = selectedPath === entry.path}
          <li>
            <div
              role="button"
              tabindex="0"
              class="group flex cursor-pointer items-center justify-between rounded px-2 py-1.5 text-xs transition-colors select-none {isSelected
                ? 'bg-primary/15 text-primary font-medium'
                : 'hover:bg-muted text-foreground/90'}"
              onclick={() => handleSelectEntry(entry)}
              ondblclick={() => handleOpenEntry(entry)}
              onkeydown={(event) => {
                if (event.key === "Enter") handleOpenEntry(entry);
                if (event.key === " ") {
                  event.preventDefault();
                  handleSelectEntry(entry);
                }
              }}
            >
              <div class="flex min-w-0 flex-1 items-center gap-2">
                <FolderIcon class="size-3.5 shrink-0 text-amber-500/80" />
                <span class="truncate font-mono text-xs">{entry.name}</span>
                {#if entry.is_hidden}
                  <span class="text-muted-foreground text-[10px] opacity-60">
                    {m.fs_hidden()}
                  </span>
                {/if}
              </div>
              {#if entry.is_dir}
                <button
                  type="button"
                  class="hover:bg-background/80 text-muted-foreground hover:text-foreground rounded p-1 opacity-0 transition-opacity group-hover:opacity-100"
                  title={m.fs_open_folder()}
                  onclick={(event) => {
                    event.stopPropagation();
                    handleOpenEntry(entry);
                  }}
                >
                  <ChevronRightIcon class="size-3.5" />
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>
