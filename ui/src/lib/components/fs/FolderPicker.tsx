import { For, Show, createEffect, createSignal } from "solid-js";
import type { JSX } from "@solidjs/web";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { buildBreadcrumbs, matchesFolderFilter, type QuickPath } from "#lib/folder-picker.ts";
import type { FsEntry } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import IconArrowUp from "~icons/lucide/arrow-up";
import IconChevronRight from "~icons/lucide/chevron-right";
import IconCornerDownLeft from "~icons/lucide/corner-down-left";
import IconEye from "~icons/lucide/eye";
import IconEyeOff from "~icons/lucide/eye-off";
import IconFolder from "~icons/lucide/folder";
import IconHardDrive from "~icons/lucide/hard-drive";
import IconHouse from "~icons/lucide/house";
import IconLoader2 from "~icons/lucide/loader-2";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconSearch from "~icons/lucide/search";

export interface FolderPickerProps {
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
  children?: JSX.Element;
}

/** Node-side folder browser used by the project / worktree pickers. */
export default function FolderPicker(props: FolderPickerProps) {
  const [currentPath, setCurrentPath] = createSignal("");
  const [parentPath, setParentPath] = createSignal<string | null>(null);
  const [entries, setEntries] = createSignal<FsEntry[]>([]);
  const [selectedPath, setSelectedPath] = createSignal("");
  const [inputPath, setInputPath] = createSignal("");
  const [filter, setFilter] = createSignal("");
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [truncated, setTruncated] = createSignal(false);
  const [showHidden, setShowHidden] = createSignal(false);
  let loadGeneration = 0;

  const pickerMode = () => props.mode ?? "select";
  const pickerFiles = () => props.files ?? false;
  const quickPaths = () => props.quickPaths ?? [];

  createEffect(
    () => ({ active: props.active ?? true, nodeId: props.nodeId, initialPath: props.initialPath }),
    (source) => {
      if (!source.active || source.nodeId === "") return;
      void loadDirectory(source.initialPath || undefined);
    },
  );

  async function loadDirectory(path?: string) {
    const node = props.nodeId;
    if (!node) return;
    const token = ++loadGeneration;
    setLoading(true);
    setError(null);
    props.onloadingchange?.(true);
    try {
      const res = await sync.connection.client.browseFs(node, path);
      if (token !== loadGeneration) return;
      setCurrentPath(res.current_path);
      setParentPath(res.parent_path);
      setEntries(res.entries);
      setTruncated(res.truncated);
      setInputPath(res.current_path);
      setFilter("");
      const selection = pickerMode() === "select" ? res.current_path : "";
      setSelectedPath(selection);
      props.onselectionchange?.(selection);
      props.onpathchange?.(res.current_path);
    } catch (cause) {
      if (token !== loadGeneration) return;
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      setTruncated(false);
      setSelectedPath("");
      props.onselectionchange?.("");
      props.onerror?.(message);
    } finally {
      if (token === loadGeneration) {
        setLoading(false);
        props.onloadingchange?.(false);
      }
    }
  }

  const displayedEntries = () =>
    entries().filter((entry) => {
      if (!pickerFiles() && !entry.is_dir) return false;
      if (!showHidden() && entry.is_hidden) return false;
      return matchesFolderFilter(entry.name, filter());
    });

  const breadcrumbs = () => buildBreadcrumbs(currentPath());

  function handleSelectEntry(entry: FsEntry): void {
    if (pickerMode() === "navigate" && entry.is_dir) {
      void loadDirectory(entry.path);
      return;
    }
    setSelectedPath(entry.path);
    props.onselectionchange?.(entry.path);
  }

  function handleOpenEntry(entry: FsEntry): void {
    if (entry.is_dir) void loadDirectory(entry.path);
    else handleSelectEntry(entry);
  }

  function handleGoUp(): void {
    const parent = parentPath();
    if (parent !== null) void loadDirectory(parent);
  }

  function handleGoHome(): void {
    void loadDirectory(undefined);
  }

  function handleGoDrives(): void {
    void loadDirectory("");
  }

  function handlePathSubmit(event: SubmitEvent): void {
    event.preventDefault();
    const target = inputPath().trim();
    if (target) void loadDirectory(target);
  }

  return (
    <div class={cn("flex min-h-0 flex-col gap-2", props.class ?? "")}>
      <div class="flex items-center gap-1.5">
        <Button
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          disabled={parentPath() === null || loading()}
          onClick={handleGoUp}
          title={m.fs_go_up()}
        >
          <IconArrowUp class="size-3.5" />
        </Button>
        <Button
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          disabled={loading()}
          onClick={handleGoHome}
          title={m.fs_go_home()}
        >
          <IconHouse class="size-3.5" />
        </Button>
        <Button
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          disabled={loading()}
          onClick={handleGoDrives}
          title={m.fs_go_root()}
        >
          <IconHardDrive class="size-3.5" />
        </Button>
        <form onSubmit={handlePathSubmit} class="flex min-w-0 flex-1 items-center gap-1">
          <Input
            value={inputPath()}
            onValueChange={setInputPath}
            placeholder={m.fs_path_placeholder()}
            class="h-8 min-w-0 flex-1 font-mono text-[13px]"
            disabled={loading()}
          />
          <Button
            type="submit"
            variant="secondary"
            size="sm"
            class="h-8 shrink-0 px-2 text-[13px]"
            disabled={loading() || inputPath().trim() === ""}
            title={m.fs_go_to_path()}
          >
            <IconCornerDownLeft class="size-3" />
          </Button>
        </form>
        <Button
          variant="ghost"
          size="icon"
          class="text-muted-foreground hover:text-foreground size-8 shrink-0"
          disabled={loading()}
          onClick={() => void loadDirectory(currentPath() || undefined)}
          title={m.common_refresh()}
        >
          <IconRefreshCw class={cn("size-3.5", loading() && "animate-spin")} />
        </Button>
      </div>

      <Show when={breadcrumbs().length > 0}>
        <div class="flex flex-nowrap items-center gap-0.5 overflow-x-auto font-mono text-[11px] whitespace-nowrap">
          <For each={breadcrumbs()}>
            {(crumb, index) => (
              <>
                <Show when={index() > 0 && !(index() === 1 && breadcrumbs()[0]?.label === "/")}>
                  <span class="text-muted-foreground/50">/</span>
                </Show>
                <button
                  type="button"
                  class="text-muted-foreground hover:text-foreground hover:bg-accent max-w-40 truncate rounded px-1 py-0.5"
                  onClick={() => void loadDirectory(crumb.path)}
                  title={crumb.path}
                >
                  {crumb.label}
                </button>
              </>
            )}
          </For>
        </div>
      </Show>

      <Show when={quickPaths().length > 0}>
        <div class="flex flex-nowrap items-center gap-1 overflow-x-auto">
          <span class="text-muted-foreground shrink-0 text-[11px]">{m.fs_recent_folders()}</span>
          <For each={quickPaths()}>
            {(quick) => (
              <button
                type="button"
                class="bg-muted/50 hover:bg-accent text-muted-foreground hover:text-foreground max-w-56 truncate rounded-full border px-2 py-0.5 text-[11px]"
                onClick={() => void loadDirectory(quick.path)}
                title={quick.description ?? quick.path}
              >
                {quick.label}
              </button>
            )}
          </For>
        </div>
      </Show>

      {props.children}

      <div class="flex items-center gap-1.5">
        <div class="border-input bg-surface-raised flex min-w-0 flex-1 items-center gap-1.5 rounded-md border px-2 py-1">
          <IconSearch class="text-muted-foreground size-3.5 shrink-0" />
          <input
            value={filter()}
            onInput={(event) => setFilter(event.currentTarget.value)}
            class="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-[13px] outline-none"
            placeholder={m.fs_filter_folders()}
          />
        </div>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground inline-flex shrink-0 items-center gap-1 text-[11px]"
          onClick={() => setShowHidden(!showHidden())}
          title={showHidden() ? m.fs_hidden_shown() : m.fs_show_hidden()}
        >
          <Show when={showHidden()} fallback={<IconEyeOff class="size-3.5" />}>
            <IconEye class="size-3.5" />
          </Show>
        </button>
      </div>

      <div
        class={cn(
          "bg-muted/20 flex flex-col overflow-y-auto rounded-md border p-1",
          props.listClass ?? "h-64",
        )}
      >
        <Show when={truncated()}>
          <p class="text-warning px-2 py-1 text-[11px]">{m.fs_truncated()}</p>
        </Show>
        <Show
          when={!loading()}
          fallback={
            <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2 py-12 text-[13px]">
              <IconLoader2 class="text-primary size-4 animate-spin" />
              <span>{m.fs_loading_folders()}</span>
            </div>
          }
        >
          <Show
            when={error() === null}
            fallback={
              <div class="flex flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
                <p class="text-destructive text-[13px]">{error()}</p>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 text-[13px]"
                  onClick={() => void loadDirectory(parentPath() ?? undefined)}
                >
                  {m.fs_back()}
                </Button>
              </div>
            }
          >
            <Show
              when={displayedEntries().length > 0}
              fallback={
                <div class="text-muted-foreground flex flex-1 items-center justify-center py-12 text-[13px]">
                  {filter().trim() === "" ? m.fs_no_subfolders() : m.fs_no_matching_folders()}
                </div>
              }
            >
              <ul class="m-0 flex list-none flex-col gap-0.5 p-0">
                <For each={displayedEntries()}>
                  {(entry) => {
                    const isSelected = () => selectedPath() === entry.path;
                    return (
                      <li>
                        <div
                          role="button"
                          tabindex={0}
                          class={cn(
                            "group flex cursor-pointer items-center justify-between rounded px-2 py-1.5 text-[13px] transition-colors select-none",
                            isSelected()
                              ? "bg-primary/15 text-primary font-medium"
                              : "hover:bg-accent text-foreground/90",
                          )}
                          onClick={() => handleSelectEntry(entry)}
                          onDblClick={() => handleOpenEntry(entry)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") handleOpenEntry(entry);
                            if (event.key === " ") {
                              event.preventDefault();
                              handleSelectEntry(entry);
                            }
                          }}
                        >
                          <div class="flex min-w-0 flex-1 items-center gap-2">
                            <IconFolder class="text-signal size-3.5 shrink-0" />
                            <span class="truncate font-mono text-[13px]">{entry.name}</span>
                            <Show when={entry.is_hidden}>
                              <span class="text-muted-foreground text-[11px] opacity-60">
                                {m.fs_hidden()}
                              </span>
                            </Show>
                          </div>
                          <Show when={entry.is_dir}>
                            <button
                              type="button"
                              class="hover:bg-foreground/10 text-muted-foreground hover:text-foreground rounded p-1 opacity-0 transition-opacity group-hover:opacity-100"
                              title={m.fs_open_folder()}
                              onClick={(event) => {
                                event.stopPropagation();
                                handleOpenEntry(entry);
                              }}
                            >
                              <IconChevronRight class="size-3.5" />
                            </button>
                          </Show>
                        </div>
                      </li>
                    );
                  }}
                </For>
              </ul>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}
