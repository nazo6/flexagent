import { For, Match, Show, Switch, createEffect, createSignal, untrack } from "solid-js";
import CodeView from "#lib/components/dock/CodeView.tsx";
import GitStatusBadge from "#lib/components/dock/GitStatusBadge.tsx";
import DiffViewer from "#lib/components/diff/DiffViewer.tsx";
import MarkdownText from "#lib/components/MarkdownText.tsx";
import { errorMessage } from "#lib/api/error-messages.ts";
import { dirname } from "#lib/dock/paths.ts";
import type { DockContext, DockDiffScope, DockFileMode, DockFileTab } from "#lib/stores/dock.ts";
import { dock } from "#lib/stores/dock.ts";
import { sync } from "#lib/stores/app.ts";
import type {
  WorkspaceDiffResponse,
  WorkspaceFileView,
  WorkspaceGitFileStatus,
} from "#lib/generated/bindings.ts";
import { formatBytes } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconChevronLeft from "~icons/lucide/chevron-left";
import IconEye from "~icons/lucide/eye";
import IconFileCode from "~icons/lucide/file-code";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPin from "~icons/lucide/pin";
import IconRefreshCw from "~icons/lucide/refresh-cw";

export interface FilePreviewProps {
  ctx: DockContext;
  tab: DockFileTab;
  mobile?: boolean;
  /** Mobile: show the tree again. */
  onback: () => void;
}

/** One file tab: source view, markdown preview or a diff. */
export default function FilePreview(props: FilePreviewProps) {
  const remote = () => sync.connection.role === "hub";
  const path = () => props.tab.path;
  const mode = (): DockFileMode => props.tab.mode;
  const scope = (): DockDiffScope => props.tab.scope;
  const isMarkdown = () => /\.(md|markdown|mdx)$/i.test(path());

  const [markdownPreview, setMarkdownPreview] = createSignal(
    untrack(() => props.tab.line === null),
  );
  let previewKey = untrack(() => `${props.tab.path}\u0000${props.tab.line ?? ""}`);
  // A chat link with a line opens in source view; keep the toggle available.
  createEffect(
    () => `${props.tab.path}\u0000${props.tab.line ?? ""}`,
    (key) => {
      if (key === previewKey) return;
      previewKey = key;
      setMarkdownPreview(props.tab.line === null);
    },
  );
  const [view, setView] = createSignal<WorkspaceFileView | null>(null);
  const [diff, setDiff] = createSignal<WorkspaceDiffResponse | null>(null);
  const [viewLoading, setViewLoading] = createSignal(false);
  const [diffLoading, setDiffLoading] = createSignal(false);
  const [viewError, setViewError] = createSignal<string | null>(null);
  const [diffError, setDiffError] = createSignal<string | null>(null);
  const [fileStatus, setFileStatus] = createSignal<WorkspaceGitFileStatus | null>(null);
  const [refreshToken, setRefreshToken] = createSignal(0);
  let viewGeneration = 0;
  let diffGeneration = 0;
  let statusGeneration = 0;

  const selectedDiff = () => diff()?.files.at(0) ?? null;

  const textView = () => {
    const content = view()?.content;
    return content?.kind === "text" ? content : null;
  };
  const imageView = () => {
    const content = view()?.content;
    return content?.kind === "image" ? content : null;
  };
  const tooLargeView = () => {
    const content = view()?.content;
    return content?.kind === "too_large" ? content : null;
  };

  createEffect(
    () => ({
      path: path(),
      mode: mode(),
      scope: scope(),
      token: refreshToken(),
    }),
    (current) => {
      if (current.path === "" || remote()) return;
      void loadFileStatus(current.path);
      if (current.mode === "view") {
        void loadView(current.path);
      } else {
        void loadDiff(current.path, current.scope);
      }
    },
  );

  async function loadFileStatus(filePath: string): Promise<void> {
    const token = ++statusGeneration;
    try {
      const result = await sync.connection.client.workspaceGitStatus(
        props.ctx.nodeId,
        props.ctx.localPath,
      );
      if (token !== statusGeneration) return;
      setFileStatus(
        result.entries.find((entry) => entry.absolute_path === filePath)?.status ?? null,
      );
    } catch {
      if (token === statusGeneration) setFileStatus(null);
    }
  }

  async function loadView(filePath: string): Promise<void> {
    const token = ++viewGeneration;
    setViewLoading(true);
    setViewError(null);
    try {
      const result = await sync.connection.client.workspaceFsFile(props.ctx.nodeId, filePath);
      if (token !== viewGeneration) return;
      setView(result);
    } catch (cause) {
      if (token !== viewGeneration) return;
      setView(null);
      setViewError(errorMessage(cause));
    } finally {
      if (token === viewGeneration) setViewLoading(false);
    }
  }

  async function loadDiff(filePath: string, diffScope: DockDiffScope): Promise<void> {
    const token = ++diffGeneration;
    setDiffLoading(true);
    setDiffError(null);
    try {
      const result = await sync.connection.client.workspaceGitDiff(
        props.ctx.nodeId,
        props.ctx.localPath,
        {
          scope: diffScope,
          file: filePath,
        },
      );
      if (token !== diffGeneration) return;
      setDiff(result);
    } catch (cause) {
      if (token !== diffGeneration) return;
      setDiff(null);
      setDiffError(errorMessage(cause));
    } finally {
      if (token === diffGeneration) setDiffLoading(false);
    }
  }

  function refresh(): void {
    setRefreshToken((token) => token + 1);
  }

  const loadingRow = (label: string) => (
    <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-[12px]">
      <IconLoaderCircle class="size-3.5 animate-spin" />
      {label}
    </div>
  );

  return (
    <>
      <div class="border-border flex h-9 shrink-0 items-center gap-1 border-b px-1.5">
        <Show when={props.mobile === true}>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
            title={m.dock_files_title()}
            aria-label={m.dock_files_title()}
            onClick={props.onback}
          >
            <IconChevronLeft class="size-4" />
          </button>
        </Show>

        <div class="bg-muted flex items-center rounded-md p-0.5">
          <For each={["view", "diff"] as const}>
            {(candidate) => (
              <button
                type="button"
                class={cn(
                  "rounded-sm px-2 py-0.5 text-[11px] font-medium transition-colors",
                  mode() === candidate
                    ? "bg-card text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                aria-pressed={mode() === candidate ? "true" : "false"}
                onClick={() => dock.setFileMode(props.tab.path, candidate)}
              >
                {candidate === "view" ? m.dock_file_view() : m.dock_file_diff()}
              </button>
            )}
          </For>
        </div>

        <Show when={mode() === "diff"}>
          <span class="text-muted-foreground shrink-0 text-[10px] font-medium tracking-wide uppercase">
            {scope() === "branch_base"
              ? m.dock_changes_scope_branch()
              : m.dock_changes_scope_worktree()}
          </span>
        </Show>
        <Show when={mode() === "view" && isMarkdown()}>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
            title={markdownPreview() ? m.dock_file_source() : m.dock_file_preview()}
            aria-label={markdownPreview() ? m.dock_file_source() : m.dock_file_preview()}
            onClick={() => setMarkdownPreview(!markdownPreview())}
          >
            <Show when={markdownPreview()} fallback={<IconEye class="size-3.5" />}>
              <IconFileCode class="size-3.5" />
            </Show>
          </button>
        </Show>

        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
          title={m.dock_files_refresh()}
          aria-label={m.dock_files_refresh()}
          onClick={refresh}
        >
          <IconRefreshCw class="size-3.5" />
        </button>

        <Show when={props.tab.preview}>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 items-center justify-center rounded-md transition-colors"
            title={m.dock_file_pin_tab()}
            aria-label={m.dock_file_pin_tab()}
            onClick={() => dock.pinFile(props.tab.path)}
          >
            <IconPin class="size-3.5" />
          </button>
        </Show>

        <span class="ml-auto flex shrink-0 items-center">
          <Show when={fileStatus()}>{(current) => <GitStatusBadge status={current()} />}</Show>
        </span>
      </div>

      <Switch>
        <Match when={remote()}>
          <div class="flex flex-1 items-center justify-center p-6 text-center">
            <p class="text-muted-foreground text-[12px]">{m.dock_files_remote_disabled()}</p>
          </div>
        </Match>

        <Match when={mode() === "view"}>
          <Switch>
            <Match when={viewLoading()}>{loadingRow(m.dock_file_loading())}</Match>
            <Match when={viewError() !== null}>
              <p class="text-destructive flex flex-1 items-center justify-center p-4 text-center text-[12px]">
                {viewError()}
              </p>
            </Match>
            <Match when={view() !== null}>
              <div class="min-h-0 flex-1 overflow-hidden p-2">
                <Switch>
                  <Match when={textView()}>
                    {(content) => (
                      <Show
                        when={isMarkdown() && markdownPreview()}
                        fallback={
                          <CodeView
                            path={view()?.path ?? path()}
                            text={content().text}
                            line={props.tab.line}
                          />
                        }
                      >
                        <div class="app-scroll h-full overflow-y-auto rounded-md border p-4">
                          <MarkdownText
                            text={content().text}
                            fileBaseDir={dirname(view()?.path ?? path())}
                          />
                        </div>
                      </Show>
                    )}
                  </Match>
                  <Match when={imageView()}>
                    {(content) => (
                      <div class="flex h-full items-center justify-center overflow-auto rounded-md border p-2">
                        <img
                          src={`data:${content().mime};base64,${content().data_b64}`}
                          alt={path()}
                          class="max-h-full max-w-full object-contain"
                        />
                      </div>
                    )}
                  </Match>
                  <Match when={tooLargeView()}>
                    {(content) => (
                      <p class="text-muted-foreground flex h-full items-center justify-center p-4 text-center text-[12px]">
                        {m.dock_file_too_large({ size: formatBytes(content().size) })}
                      </p>
                    )}
                  </Match>
                  <Match when={view()?.content.kind === "binary"}>
                    <p class="text-muted-foreground flex h-full items-center justify-center p-4 text-center text-[12px]">
                      {m.dock_file_binary()}
                    </p>
                  </Match>
                </Switch>
              </div>
            </Match>
          </Switch>
        </Match>

        <Match when={mode() === "diff"}>
          <Show when={diff()?.truncated === true}>
            <p class="text-warning px-2 pt-2 text-[12px]">{m.diff_truncated()}</p>
          </Show>
          <Switch>
            <Match when={diffLoading()}>{loadingRow(m.diff_loading())}</Match>
            <Match when={diffError() !== null}>
              <p class="text-destructive flex flex-1 items-center justify-center p-4 text-center text-[12px]">
                {diffError()}
              </p>
            </Match>
            <Match when={selectedDiff() === null}>
              <p class="text-muted-foreground flex flex-1 items-center justify-center p-4 text-center text-[12px]">
                {m.dock_file_no_changes()}
              </p>
            </Match>
            <Match when={selectedDiff()}>
              {(file) => (
                <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-2">
                  <DiffViewer file={file()} />
                </div>
              )}
            </Match>
          </Switch>
        </Match>
      </Switch>
    </>
  );
}
