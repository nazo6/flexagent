<script lang="ts">
  import { untrack } from "svelte";
  import FilePreview from "#lib/components/dock/FilePreview.svelte";
  import FileTabs from "#lib/components/dock/FileTabs.svelte";
  import GitStatusBadge from "#lib/components/dock/GitStatusBadge.svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { basename } from "#lib/dock/paths.ts";
  import type { DockContext } from "#lib/stores/dock.svelte.ts";
  import { dock } from "#lib/stores/dock.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import type {
    WorkspaceFsEntry,
    WorkspaceGitFileStatus,
    WorkspaceGitStatusResponse,
  } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import EyeIcon from "@lucide/svelte/icons/eye";
  import EyeOffIcon from "@lucide/svelte/icons/eye-off";
  import FileIcon from "@lucide/svelte/icons/file";
  import FileQuestionIcon from "@lucide/svelte/icons/file-question";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import FolderTreeIcon from "@lucide/svelte/icons/folder-tree";
  import LinkIcon from "@lucide/svelte/icons/link";
  import ListFilterIcon from "@lucide/svelte/icons/list-filter";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PanelLeftIcon from "@lucide/svelte/icons/panel-left";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  let { ctx, mobile = false }: { ctx: DockContext; mobile?: boolean } = $props();

  interface DirState {
    entries: WorkspaceFsEntry[];
    loading: boolean;
    error: string | null;
    truncated: boolean;
  }

  interface TreeRow {
    entry: WorkspaceFsEntry;
    depth: number;
    expanded: boolean;
    loading: boolean;
    error: string | null;
  }

  /** Heavy directories hidden by default (the node still returns them). */
  const IGNORED_NAMES = new Set([
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    "vendor",
    "venv",
    ".venv",
    ".svelte-kit",
    ".next",
    ".nuxt",
  ]);

  const remote = $derived(sync.connection.role === "hub");

  let rootPath = $state<string | null>(null);
  let dirs = $state<Record<string, DirState>>({});
  let expanded = $state<Record<string, boolean>>({});
  let status = $state<WorkspaceGitStatusResponse | null>(null);
  let showHidden = $state(false);
  let showIgnored = $state(false);
  let changedOnly = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let statusError = $state<string | null>(null);
  let refreshToken = $state(0);
  let generation = 0;

  const activePreview = $derived(dock.activeFileTab);
  const showTree = $derived(
    mobile ? dock.treeVisible || dock.activeFile === null : dock.treeVisible,
  );
  const showPreview = $derived(mobile ? !showTree && dock.activeFile !== null : true);

  const statusByPath = $derived(
    new Map<string, WorkspaceGitFileStatus>(
      (status?.entries ?? []).map((entry) => [entry.absolute_path, entry.status]),
    ),
  );
  const rootName = $derived(rootPath === null ? "" : basename(rootPath));
  const listingTruncated = $derived(Object.values(dirs).some((dir) => dir.truncated));

  const rows = $derived.by(() => {
    const result: TreeRow[] = [];
    if (rootPath === null) return result;
    const walk = (path: string, depth: number): void => {
      const dir = dirs[path];
      if (dir === undefined) return;
      for (const entry of visibleEntries(dir.entries)) {
        const isExpanded = entry.kind === "directory" && expanded[entry.path] === true;
        const child = dirs[entry.path];
        result.push({
          entry,
          depth,
          expanded: isExpanded,
          loading: child?.loading ?? false,
          error: child?.error ?? null,
        });
        if (isExpanded) walk(entry.path, depth + 1);
      }
    };
    walk(rootPath, 0);
    return result;
  });

  $effect(() => {
    const key = `${ctx.nodeId}\u0000${ctx.localPath}`;
    const token = refreshToken;
    void key;
    void token;
    if (remote) return;
    void reload();
  });

  // On mobile the tree and the preview share one pane. Every openFile request
  // reveals the preview; the back button brings the tree back. Tracking the
  // request counter (not only the path) also covers reopening the same file,
  // while the initial run preserves the persisted tree/preview choice.
  let lastOpenVersion = dock.fileOpenVersion;
  $effect(() => {
    const version = dock.fileOpenVersion;
    if (version === lastOpenVersion) return;
    lastOpenVersion = version;
    if (!mobile || dock.activeFile === null) return;
    if (untrack(() => dock.treeVisible)) {
      dock.setTreeVisible(false);
    }
  });

  function visibleEntries(entries: WorkspaceFsEntry[]): WorkspaceFsEntry[] {
    return entries.filter((entry) => {
      if (!showHidden && entry.is_hidden) return false;
      if (!showIgnored && IGNORED_NAMES.has(entry.name.toLowerCase())) return false;
      return true;
    });
  }

  async function reload(): Promise<void> {
    const token = ++generation;
    loading = true;
    error = null;
    statusError = null;
    rootPath = null;
    dirs = {};
    expanded = {};
    status = null;
    try {
      const client = sync.connection.client;
      const first = await client.workspaceFsList(ctx.nodeId, ctx.localPath);
      if (token !== generation) return;
      const root = first.root_path;
      const rootListing =
        root === first.path ? first : await client.workspaceFsList(ctx.nodeId, root);
      if (token !== generation) return;
      rootPath = root;
      dirs = {
        [root]: {
          entries: rootListing.entries,
          loading: false,
          error: null,
          truncated: rootListing.truncated,
        },
      };
      loading = false;
      void loadStatus(token);
    } catch (cause) {
      if (token !== generation) return;
      error = errorMessage(cause);
      loading = false;
    }
  }

  async function loadStatus(token: number): Promise<void> {
    try {
      const result = await sync.connection.client.workspaceGitStatus(ctx.nodeId, ctx.localPath);
      if (token !== generation) return;
      status = result;
    } catch (cause) {
      if (token !== generation) return;
      statusError = errorMessage(cause);
    }
  }

  async function loadDir(path: string, token: number): Promise<void> {
    const previous = dirs[path];
    dirs = {
      ...dirs,
      [path]: {
        entries: previous?.entries ?? [],
        loading: true,
        error: null,
        truncated: previous?.truncated ?? false,
      },
    };
    try {
      const result = await sync.connection.client.workspaceFsList(ctx.nodeId, path);
      if (token !== generation) return;
      dirs = {
        ...dirs,
        [path]: {
          entries: result.entries,
          loading: false,
          error: null,
          truncated: result.truncated,
        },
      };
    } catch (cause) {
      if (token !== generation) return;
      dirs = {
        ...dirs,
        [path]: {
          entries: previous?.entries ?? [],
          loading: false,
          error: errorMessage(cause),
          truncated: false,
        },
      };
    }
  }

  function toggleDir(entry: WorkspaceFsEntry): void {
    const open = expanded[entry.path] !== true;
    expanded = { ...expanded, [entry.path]: open };
    if (open && dirs[entry.path] === undefined) {
      void loadDir(entry.path, generation);
    }
  }

  function openEntry(entry: WorkspaceFsEntry): void {
    if (entry.kind === "directory") {
      toggleDir(entry);
      return;
    }
    dock.openFile(entry.path, "view");
  }

  function openChanged(entry: { absolute_path: string }): void {
    dock.openFile(entry.absolute_path, "diff");
  }

  function refresh(): void {
    refreshToken += 1;
  }

  function isDirectory(entry: WorkspaceFsEntry): boolean {
    return entry.kind === "directory";
  }
</script>

{#if remote}
  <div class="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
    <FolderIcon class="text-muted-foreground size-8" />
    <p class="text-[13px] font-medium">{m.dock_files_remote_disabled()}</p>
    <p class="text-muted-foreground text-[12px] leading-relaxed">
      {m.dock_files_remote_hint()}
    </p>
  </div>
{:else}
  <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
    {#if !(mobile && showPreview)}
      <div class="border-border flex h-9 shrink-0 items-center gap-0.5 border-b px-1.5">
        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors disabled:opacity-50"
          title={m.dock_files_refresh()}
          aria-label={m.dock_files_refresh()}
          disabled={loading}
          onclick={refresh}
        >
          <RefreshCwIcon class="size-3.5" />
        </button>
        <button
          type="button"
          class={cn(
            "flex size-7 items-center justify-center rounded-md transition-colors",
            showHidden
              ? "bg-signal-soft text-signal"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
          title={showHidden ? m.dock_files_hide_hidden() : m.dock_files_show_hidden()}
          aria-label={showHidden ? m.dock_files_hide_hidden() : m.dock_files_show_hidden()}
          aria-pressed={showHidden}
          onclick={() => (showHidden = !showHidden)}
        >
          {#if showHidden}
            <EyeIcon class="size-3.5" />
          {:else}
            <EyeOffIcon class="size-3.5" />
          {/if}
        </button>
        <button
          type="button"
          class={cn(
            "flex size-7 items-center justify-center rounded-md transition-colors",
            showIgnored
              ? "bg-signal-soft text-signal"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
          title={showIgnored ? m.dock_files_hide_ignored() : m.dock_files_show_ignored()}
          aria-label={showIgnored ? m.dock_files_hide_ignored() : m.dock_files_show_ignored()}
          aria-pressed={showIgnored}
          onclick={() => (showIgnored = !showIgnored)}
        >
          <FolderTreeIcon class="size-3.5" />
        </button>
        <button
          type="button"
          class={cn(
            "flex size-7 items-center justify-center rounded-md transition-colors",
            changedOnly
              ? "bg-signal-soft text-signal"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
          title={m.dock_files_changed_only()}
          aria-label={m.dock_files_changed_only()}
          aria-pressed={changedOnly}
          onclick={() => (changedOnly = !changedOnly)}
        >
          <ListFilterIcon class="size-3.5" />
        </button>
        {#if !mobile}
          <button
            type="button"
            class={cn(
              "flex size-7 items-center justify-center rounded-md transition-colors",
              dock.treeVisible
                ? "text-muted-foreground hover:bg-accent hover:text-foreground"
                : "bg-signal-soft text-signal",
            )}
            title={dock.treeVisible ? m.dock_files_hide_tree() : m.dock_files_show_tree()}
            aria-label={dock.treeVisible ? m.dock_files_hide_tree() : m.dock_files_show_tree()}
            aria-pressed={!dock.treeVisible}
            onclick={() => dock.setTreeVisible(!dock.treeVisible)}
          >
            <PanelLeftIcon class="size-3.5" />
          </button>
        {/if}
        <span
          class="text-muted-foreground ml-auto min-w-0 truncate pl-2 font-mono text-[11px]"
          title={rootPath ?? ""}
        >
          {rootName}
        </span>
      </div>
    {/if}

    <div class="flex min-h-0 flex-1">
      {#if showTree}
        <div
          class={cn(
            "flex min-h-0 flex-col",
            mobile ? "w-full" : "w-[38%] min-w-[180px] max-w-[320px] border-r border-border",
          )}
        >
          <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-1">
            {#if error !== null}
              <p class="text-destructive p-2 text-center text-[12px]">{error}</p>
            {:else if loading}
              <div
                class="text-muted-foreground flex items-center justify-center gap-2 p-4 text-[12px]"
              >
                <LoaderCircleIcon class="size-3.5 animate-spin" />
                {m.dock_files_loading()}
              </div>
            {:else}
              {#if statusError !== null}
                <p class="text-muted-foreground px-1.5 py-1 text-[11px]">{statusError}</p>
              {/if}

              {#if changedOnly}
                {#if statusError !== null}
                  <p class="text-destructive p-2 text-[12px]">{statusError}</p>
                {:else if status === null}
                  <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_loading()}</p>
                {:else if !status.is_repository}
                  <p class="text-muted-foreground p-2 text-[12px]">
                    {m.dock_changes_not_repository()}
                  </p>
                {:else if status.entries.length === 0}
                  <p class="text-muted-foreground p-2 text-[12px]">{m.diff_no_changes()}</p>
                {:else}
                  {#each status.entries as entry (entry.absolute_path)}
                    <button
                      type="button"
                      class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
                      title={entry.path}
                      onclick={() => openChanged(entry)}
                    >
                      <GitStatusBadge status={entry.status} />
                      <span class="min-w-0 flex-1 truncate font-mono">{entry.path}</span>
                    </button>
                  {/each}
                {/if}
              {:else if rows.length === 0}
                <p class="text-muted-foreground p-2 text-[12px]">{m.dock_files_empty()}</p>
              {:else}
                {#each rows as row (row.entry.path)}
                  {@const badge = statusByPath.get(row.entry.path)}
                  <button
                    type="button"
                    class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm py-1 pr-1.5 text-left text-[12px]"
                    style={`padding-left: ${row.depth * 12 + 6}px`}
                    title={row.entry.path}
                    onclick={() => openEntry(row.entry)}
                  >
                    {#if isDirectory(row.entry)}
                      {#if row.loading}
                        <LoaderCircleIcon
                          class="text-muted-foreground size-3.5 shrink-0 animate-spin"
                        />
                      {:else if row.expanded}
                        <ChevronDownIcon class="text-muted-foreground size-3.5 shrink-0" />
                      {:else}
                        <ChevronRightIcon class="text-muted-foreground size-3.5 shrink-0" />
                      {/if}
                    {:else}
                      <span class="size-3.5 shrink-0"></span>
                    {/if}

                    {#if isDirectory(row.entry)}
                      <FolderIcon class="text-muted-foreground size-3.5 shrink-0" />
                    {:else if row.entry.kind === "symlink"}
                      <LinkIcon class="text-muted-foreground size-3.5 shrink-0" />
                    {:else if row.entry.kind === "file"}
                      <FileIcon class="text-muted-foreground size-3.5 shrink-0" />
                    {:else}
                      <FileQuestionIcon class="text-muted-foreground size-3.5 shrink-0" />
                    {/if}

                    <span class="min-w-0 flex-1 truncate">{row.entry.name}</span>
                    {#if badge !== undefined}
                      <GitStatusBadge status={badge} />
                    {/if}
                  </button>
                  {#if row.error !== null}
                    <p
                      class="text-destructive pb-1 text-[11px]"
                      style={`padding-left: ${row.depth * 12 + 30}px`}
                    >
                      {row.error}
                    </p>
                  {/if}
                {/each}
              {/if}

              {#if listingTruncated}
                <p class="text-muted-foreground px-1.5 py-1 text-[11px]">
                  {m.dock_files_truncated()}
                </p>
              {/if}
            {/if}
          </div>
        </div>
      {/if}

      {#if showPreview}
        <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
          <FileTabs
            tabs={dock.files}
            activePath={dock.activeFile}
            onactivate={(path) => dock.activateFile(path)}
            onclose={(path) => dock.closeFile(path)}
          />
          {#if activePreview !== null}
            <FilePreview
              {ctx}
              tab={activePreview}
              {mobile}
              onback={() => dock.setTreeVisible(true)}
            />
          {:else}
            <div
              class="text-muted-foreground flex flex-1 items-center justify-center p-4 text-center text-[12px]"
            >
              {m.diff_select_file()}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}
