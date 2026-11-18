import { For, Show, createEffect, createMemo, createSignal, untrack } from "solid-js";
import FilePreview from "#lib/components/dock/FilePreview.tsx";
import FileTabs from "#lib/components/dock/FileTabs.tsx";
import GitStatusBadge from "#lib/components/dock/GitStatusBadge.tsx";
import { errorMessage } from "#lib/api/error-messages.ts";
import { basename } from "#lib/dock/paths.ts";
import type { DockContext } from "#lib/stores/dock.ts";
import { dock, MAX_TREE_WIDTH, MIN_TREE_WIDTH } from "#lib/stores/dock.ts";
import { sync } from "#lib/stores/app.ts";
import type {
  WorkspaceFsEntry,
  WorkspaceGitFileStatus,
  WorkspaceGitStatusResponse,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronRight from "~icons/lucide/chevron-right";
import IconEye from "~icons/lucide/eye";
import IconEyeOff from "~icons/lucide/eye-off";
import IconFile from "~icons/lucide/file";
import IconFileQuestion from "~icons/lucide/file-question";
import IconFolder from "~icons/lucide/folder";
import IconFolderTree from "~icons/lucide/folder-tree";
import IconLink from "~icons/lucide/link";
import IconListFilter from "~icons/lucide/list-filter";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPanelLeft from "~icons/lucide/panel-left";
import IconRefreshCw from "~icons/lucide/refresh-cw";

export interface FilesToolProps {
  ctx: DockContext;
  mobile?: boolean;
}

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

/** Files tree, changed-file filter and preview split of the Files tool. */
export default function FilesTool(props: FilesToolProps) {
  const isMobile = () => props.mobile === true;

  const [rootPath, setRootPath] = createSignal<string | null>(null);
  const [dirs, setDirs] = createSignal<Record<string, DirState>>({});
  const [expanded, setExpanded] = createSignal<Record<string, boolean>>({});
  const [status, setStatus] = createSignal<WorkspaceGitStatusResponse | null>(null);
  const [showHidden, setShowHidden] = createSignal(false);
  const [showIgnored, setShowIgnored] = createSignal(false);
  const [changedOnly, setChangedOnly] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [statusError, setStatusError] = createSignal<string | null>(null);
  const [refreshToken, setRefreshToken] = createSignal(0);
  let generation = 0;

  /** Files split: live drag width until release, then commit to the store. */
  const [splitContainer, setSplitContainer] = createSignal<HTMLDivElement>();
  const [draggingTree, setDraggingTree] = createSignal(false);
  const [dragTreeWidth, setDragTreeWidth] = createSignal<number | null>(null);
  const treeWidth = () => dragTreeWidth() ?? dock.treeWidth;

  const activePreview = () => dock.activeFileTab;
  const showTree = () =>
    isMobile() ? dock.treeVisible || dock.activeFile === null : dock.treeVisible;
  const showPreview = () => (isMobile() ? !showTree() && dock.activeFile !== null : true);

  const statusByPath = createMemo(
    () =>
      new Map<string, WorkspaceGitFileStatus>(
        (status()?.entries ?? []).map((entry) => [entry.absolute_path, entry.status]),
      ),
  );
  const rootName = () => {
    const root = rootPath();
    return root === null ? "" : basename(root);
  };
  const listingTruncated = () => Object.values(dirs()).some((dir) => dir.truncated);

  const rows = createMemo(() => {
    const result: TreeRow[] = [];
    const root = rootPath();
    if (root === null) return result;
    const visible = visibleEntries;
    const walk = (path: string, depth: number): void => {
      const dir = dirs()[path];
      if (dir === undefined) return;
      for (const entry of visible(dir.entries)) {
        const isExpanded = entry.kind === "directory" && expanded()[entry.path] === true;
        const child = dirs()[entry.path];
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
    walk(root, 0);
    return result;
  });

  createEffect(
    () => ({ key: `${props.ctx.nodeId}\u0000${props.ctx.localPath}`, token: refreshToken() }),
    () => {
      void reload();
    },
  );

  // On mobile the tree and the preview share one pane. Every openFile request
  // reveals the preview; the back button brings the tree back. Tracking the
  // request counter (not only the path) also covers reopening the same file,
  // while the initial run preserves the persisted tree/preview choice.
  let lastOpenVersion = dock.fileOpenVersion;
  createEffect(
    () => dock.fileOpenVersion,
    (version) => {
      if (version === lastOpenVersion) return;
      lastOpenVersion = version;
      if (!isMobile() || dock.activeFile === null) return;
      if (untrack(() => dock.treeVisible)) {
        dock.setTreeVisible(false);
      }
    },
  );

  function visibleEntries(entries: WorkspaceFsEntry[]): WorkspaceFsEntry[] {
    return entries.filter((entry) => {
      if (!showHidden() && entry.is_hidden) return false;
      if (!showIgnored() && IGNORED_NAMES.has(entry.name.toLowerCase())) return false;
      return true;
    });
  }

  async function reload(): Promise<void> {
    const token = ++generation;
    setLoading(true);
    setError(null);
    setStatusError(null);
    setRootPath(null);
    setDirs({});
    setExpanded({});
    setStatus(null);
    try {
      const client = sync.connection.client;
      const first = await client.workspaceFsList(props.ctx.nodeId, props.ctx.localPath);
      if (token !== generation) return;
      const root = first.root_path;
      const rootListing =
        root === first.path ? first : await client.workspaceFsList(props.ctx.nodeId, root);
      if (token !== generation) return;
      setRootPath(root);
      setDirs({
        [root]: {
          entries: rootListing.entries,
          loading: false,
          error: null,
          truncated: rootListing.truncated,
        },
      });
      setLoading(false);
      void loadStatus(token);
    } catch (cause) {
      if (token !== generation) return;
      setError(errorMessage(cause));
      setLoading(false);
    }
  }

  async function loadStatus(token: number): Promise<void> {
    try {
      const result = await sync.connection.client.workspaceGitStatus(
        props.ctx.nodeId,
        props.ctx.localPath,
      );
      if (token !== generation) return;
      setStatus(result);
    } catch (cause) {
      if (token !== generation) return;
      setStatusError(errorMessage(cause));
    }
  }

  async function loadDir(path: string, token: number): Promise<void> {
    const previous = dirs()[path];
    setDirs({
      ...dirs(),
      [path]: {
        entries: previous?.entries ?? [],
        loading: true,
        error: null,
        truncated: previous?.truncated ?? false,
      },
    });
    try {
      const result = await sync.connection.client.workspaceFsList(props.ctx.nodeId, path);
      if (token !== generation) return;
      setDirs({
        ...dirs(),
        [path]: {
          entries: result.entries,
          loading: false,
          error: null,
          truncated: result.truncated,
        },
      });
    } catch (cause) {
      if (token !== generation) return;
      setDirs({
        ...dirs(),
        [path]: {
          entries: previous?.entries ?? [],
          loading: false,
          error: errorMessage(cause),
          truncated: false,
        },
      });
    }
  }

  function toggleDir(entry: WorkspaceFsEntry): void {
    const open = expanded()[entry.path] !== true;
    setExpanded({ ...expanded(), [entry.path]: open });
    if (open && dirs()[entry.path] === undefined) {
      void loadDir(entry.path, generation);
    }
  }

  function openEntry(entry: WorkspaceFsEntry): void {
    if (entry.kind === "directory") {
      toggleDir(entry);
      return;
    }
    dock.openFile(entry.path);
  }

  /** Double-click pins the tab so later preview opens keep it. */
  function openEntryPinned(entry: WorkspaceFsEntry): void {
    if (entry.kind === "directory") return;
    dock.openFile(entry.path, { preview: false });
  }

  function openChanged(entry: { absolute_path: string }): void {
    dock.openFile(entry.absolute_path, { mode: "diff" });
  }

  function openChangedPinned(entry: { absolute_path: string }): void {
    dock.openFile(entry.absolute_path, { mode: "diff", preview: false });
  }

  function refresh(): void {
    setRefreshToken((token) => token + 1);
  }

  function startResize(event: PointerEvent & { currentTarget: HTMLElement }): void {
    if (event.button !== 0) return;
    event.preventDefault();
    setDraggingTree(true);
    setDragTreeWidth(dock.treeWidth);
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function moveResize(event: PointerEvent): void {
    const holder = splitContainer();
    if (!draggingTree() || holder === undefined) return;
    const rect = holder.getBoundingClientRect();
    if (rect.width <= 0) return;
    const percent = ((event.clientX - rect.left) / rect.width) * 100;
    setDragTreeWidth(Math.min(MAX_TREE_WIDTH, Math.max(MIN_TREE_WIDTH, percent)));
  }

  function endResize(event: PointerEvent & { currentTarget: HTMLElement }): void {
    if (!draggingTree()) return;
    setDraggingTree(false);
    const element = event.currentTarget;
    if (element.hasPointerCapture(event.pointerId)) {
      element.releasePointerCapture(event.pointerId);
    }
    const width = dragTreeWidth();
    if (width !== null) dock.setTreeWidth(width);
    setDragTreeWidth(null);
  }

  function isDirectory(entry: WorkspaceFsEntry): boolean {
    return entry.kind === "directory";
  }

  const toggleClass = (on: boolean) =>
    on
      ? "bg-signal-soft text-signal"
      : "text-muted-foreground hover:bg-accent hover:text-foreground";

  return (
    <div class="flex min-h-0 flex-1 flex-col overflow-hidden">
      <Show when={!(isMobile() && showPreview())}>
        <div class="border-border flex h-9 shrink-0 items-center gap-0.5 border-b px-1.5">
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors disabled:opacity-50"
            title={m.dock_files_refresh()}
            aria-label={m.dock_files_refresh()}
            disabled={loading()}
            onClick={refresh}
          >
            <IconRefreshCw class="size-3.5" />
          </button>
          <button
            type="button"
            class={cn(
              "flex size-7 items-center justify-center rounded-md transition-colors",
              toggleClass(showHidden()),
            )}
            title={showHidden() ? m.dock_files_hide_hidden() : m.dock_files_show_hidden()}
            aria-label={showHidden() ? m.dock_files_hide_hidden() : m.dock_files_show_hidden()}
            aria-pressed={showHidden() ? "true" : "false"}
            onClick={() => setShowHidden(!showHidden())}
          >
            <Show when={showHidden()} fallback={<IconEyeOff class="size-3.5" />}>
              <IconEye class="size-3.5" />
            </Show>
          </button>
          <button
            type="button"
            class={cn(
              "flex size-7 items-center justify-center rounded-md transition-colors",
              toggleClass(showIgnored()),
            )}
            title={showIgnored() ? m.dock_files_hide_ignored() : m.dock_files_show_ignored()}
            aria-label={showIgnored() ? m.dock_files_hide_ignored() : m.dock_files_show_ignored()}
            aria-pressed={showIgnored() ? "true" : "false"}
            onClick={() => setShowIgnored(!showIgnored())}
          >
            <IconFolderTree class="size-3.5" />
          </button>
          <button
            type="button"
            class={cn(
              "flex size-7 items-center justify-center rounded-md transition-colors",
              toggleClass(changedOnly()),
            )}
            title={m.dock_files_changed_only()}
            aria-label={m.dock_files_changed_only()}
            aria-pressed={changedOnly() ? "true" : "false"}
            onClick={() => setChangedOnly(!changedOnly())}
          >
            <IconListFilter class="size-3.5" />
          </button>
          <Show when={!isMobile()}>
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
              aria-pressed={dock.treeVisible ? "false" : "true"}
              onClick={() => dock.setTreeVisible(!dock.treeVisible)}
            >
              <IconPanelLeft class="size-3.5" />
            </button>
          </Show>
          <span
            class="text-muted-foreground ml-auto min-w-0 truncate pl-2 font-mono text-[11px]"
            title={rootPath() ?? ""}
          >
            {rootName()}
          </span>
        </div>
      </Show>

      <div ref={(element) => setSplitContainer(() => element)} class="flex min-h-0 flex-1">
        <Show when={showTree()}>
          <div
            class={cn(
              "flex min-h-0 flex-col",
              isMobile() ? "w-full" : "min-w-[160px] max-w-[70%] shrink-0 border-r border-border",
            )}
            style={isMobile() ? undefined : `width:${treeWidth()}%`}
          >
            <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-1">
              <Show
                when={error() === null}
                fallback={<p class="text-destructive p-2 text-center text-[12px]">{error()}</p>}
              >
                <Show
                  when={!loading()}
                  fallback={
                    <div class="text-muted-foreground flex items-center justify-center gap-2 p-4 text-[12px]">
                      <IconLoaderCircle class="size-3.5 animate-spin" />
                      {m.dock_files_loading()}
                    </div>
                  }
                >
                  <Show when={statusError() !== null}>
                    <p class="text-muted-foreground px-1.5 py-1 text-[11px]">{statusError()}</p>
                  </Show>

                  <Show
                    when={changedOnly()}
                    fallback={
                      <Show
                        when={rows().length > 0}
                        fallback={
                          <p class="text-muted-foreground p-2 text-[12px]">
                            {m.dock_files_empty()}
                          </p>
                        }
                      >
                        <For each={rows()} keyed={(row) => row.entry.path}>
                          {(row) => {
                            const badge = () => statusByPath().get(row().entry.path);
                            return (
                              <>
                                <button
                                  type="button"
                                  class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm py-1 pr-1.5 text-left text-[12px]"
                                  style={`padding-left: ${row().depth * 12 + 6}px`}
                                  title={row().entry.path}
                                  onClick={() => openEntry(row().entry)}
                                  onDblClick={() => openEntryPinned(row().entry)}
                                >
                                  <Show
                                    when={isDirectory(row().entry)}
                                    fallback={<span class="size-3.5 shrink-0"></span>}
                                  >
                                    <Show
                                      when={!row().loading}
                                      fallback={
                                        <IconLoaderCircle class="text-muted-foreground size-3.5 shrink-0 animate-spin" />
                                      }
                                    >
                                      <Show
                                        when={row().expanded}
                                        fallback={
                                          <IconChevronRight class="text-muted-foreground size-3.5 shrink-0" />
                                        }
                                      >
                                        <IconChevronDown class="text-muted-foreground size-3.5 shrink-0" />
                                      </Show>
                                    </Show>
                                  </Show>

                                  <Show
                                    when={isDirectory(row().entry)}
                                    fallback={
                                      <Show
                                        when={row().entry.kind === "symlink"}
                                        fallback={
                                          <Show
                                            when={row().entry.kind === "file"}
                                            fallback={
                                              <IconFileQuestion class="text-muted-foreground size-3.5 shrink-0" />
                                            }
                                          >
                                            <IconFile class="text-muted-foreground size-3.5 shrink-0" />
                                          </Show>
                                        }
                                      >
                                        <IconLink class="text-muted-foreground size-3.5 shrink-0" />
                                      </Show>
                                    }
                                  >
                                    <IconFolder class="text-muted-foreground size-3.5 shrink-0" />
                                  </Show>

                                  <span class="min-w-0 flex-1 truncate">{row().entry.name}</span>
                                  <Show when={badge()}>
                                    {(current) => <GitStatusBadge status={current()} />}
                                  </Show>
                                </button>
                                <Show when={row().error !== null}>
                                  <p
                                    class="text-destructive pb-1 text-[11px]"
                                    style={`padding-left: ${row().depth * 12 + 30}px`}
                                  >
                                    {row().error}
                                  </p>
                                </Show>
                              </>
                            );
                          }}
                        </For>
                      </Show>
                    }
                  >
                    <Show
                      when={statusError() === null}
                      fallback={<p class="text-destructive p-2 text-[12px]">{statusError()}</p>}
                    >
                      <Show
                        when={status() !== null}
                        fallback={
                          <p class="text-muted-foreground p-2 text-[12px]">
                            {m.dock_changes_loading()}
                          </p>
                        }
                      >
                        <Show
                          when={status()?.is_repository === true}
                          fallback={
                            <p class="text-muted-foreground p-2 text-[12px]">
                              {m.dock_changes_not_repository()}
                            </p>
                          }
                        >
                          <Show
                            when={(status()?.entries.length ?? 0) > 0}
                            fallback={
                              <p class="text-muted-foreground p-2 text-[12px]">
                                {m.diff_no_changes()}
                              </p>
                            }
                          >
                            <For
                              each={status()?.entries ?? []}
                              keyed={(entry) => entry.absolute_path}
                            >
                              {(entry) => (
                                <button
                                  type="button"
                                  class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
                                  title={entry().path}
                                  onClick={() => openChanged(entry())}
                                  onDblClick={() => openChangedPinned(entry())}
                                >
                                  <GitStatusBadge status={entry().status} />
                                  <span class="min-w-0 flex-1 truncate font-mono">
                                    {entry().path}
                                  </span>
                                </button>
                              )}
                            </For>
                          </Show>
                        </Show>
                      </Show>
                    </Show>
                  </Show>

                  <Show when={listingTruncated()}>
                    <p class="text-muted-foreground px-1.5 py-1 text-[11px]">
                      {m.dock_files_truncated()}
                    </p>
                  </Show>
                </Show>
              </Show>
            </div>
          </div>
        </Show>

        <Show when={showTree() && !isMobile()}>
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label={m.dock_files_resize_tree()}
            class={cn(
              "group relative w-1.5 shrink-0 cursor-col-resize touch-none transition-colors",
              draggingTree() ? "bg-signal/50" : "hover:bg-accent",
            )}
            onPointerDown={startResize}
            onPointerMove={moveResize}
            onPointerUp={endResize}
            onPointerCancel={endResize}
          >
            <span class="bg-border absolute inset-y-0 left-1/2 w-px -translate-x-1/2"></span>
          </div>
        </Show>

        <Show when={showPreview()}>
          <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
            <FileTabs
              tabs={dock.files}
              activePath={dock.activeFile}
              onactivate={(path) => dock.activateFile(path)}
              onclose={(path) => dock.closeFile(path)}
              onpin={(path) => dock.pinFile(path)}
            />
            <Show
              when={activePreview()}
              fallback={
                <div class="text-muted-foreground flex flex-1 items-center justify-center p-4 text-center text-[12px]">
                  {m.diff_select_file()}
                </div>
              }
            >
              {(tab) => (
                <FilePreview
                  ctx={props.ctx}
                  tab={tab()}
                  mobile={isMobile()}
                  onback={() => dock.setTreeVisible(true)}
                />
              )}
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}
