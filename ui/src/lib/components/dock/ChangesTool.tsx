import { For, Match, Show, Switch, createEffect, createMemo, createSignal } from "solid-js";
import GitStatusBadge from "#lib/components/dock/GitStatusBadge.tsx";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import { ApiError } from "#lib/api/errors.ts";
import { joinPath } from "#lib/dock/paths.ts";
import type { DockContext, DockDiffScope } from "#lib/stores/dock.ts";
import { dock } from "#lib/stores/dock.ts";
import { sync } from "#lib/stores/app.ts";
import type {
  FileDiff,
  WorkspaceDiffResponse,
  WorkspaceGitFileStatus,
  WorkspaceGitStatusResponse,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import IconArrowDownToLine from "~icons/lucide/arrow-down-to-line";
import IconArrowUpFromLine from "~icons/lucide/arrow-up-from-line";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconRefreshCw from "~icons/lucide/refresh-cw";

export interface ChangesToolProps {
  ctx: DockContext;
  items: TimelineItem[];
}

type Scope = "session" | "uncommitted" | "branch_base";

/** Session / worktree changes, commit box and git sync controls of the dock. */
export default function ChangesTool(props: ChangesToolProps) {
  const [scope, setScope] = createSignal<Scope>("session");
  const [status, setStatus] = createSignal<WorkspaceGitStatusResponse | null>(null);
  const [branchDiff, setBranchDiff] = createSignal<WorkspaceDiffResponse | null>(null);
  const [statusLoading, setStatusLoading] = createSignal(false);
  const [branchLoading, setBranchLoading] = createSignal(false);
  const [statusError, setStatusError] = createSignal<string | null>(null);
  const [branchError, setBranchError] = createSignal<string | null>(null);
  const [refreshToken, setRefreshToken] = createSignal(0);
  let statusGeneration = 0;
  let branchGeneration = 0;
  const [commitMessage, setCommitMessage] = createSignal("");
  const [committing, setCommitting] = createSignal(false);
  const [syncAction, setSyncAction] = createSignal<"pull" | "push" | null>(null);

  const canCommit = createMemo(() => {
    const current = status();
    return (
      scope() === "uncommitted" &&
      current !== null &&
      current.is_repository &&
      current.entries.length > 0 &&
      commitMessage().trim() !== "" &&
      !committing()
    );
  });

  const canSync = createMemo(() => {
    const current = status();
    return (
      scope() === "uncommitted" &&
      current !== null &&
      current.is_repository &&
      current.branch !== null &&
      syncAction() === null
    );
  });

  const scopes: { value: Scope; label: string }[] = [
    { value: "session", label: m.dock_changes_scope_session() },
    { value: "uncommitted", label: m.dock_changes_scope_worktree() },
    { value: "branch_base", label: m.dock_changes_scope_branch() },
  ];

  /** Files edited by this session's tool calls (last write wins). */
  const sessionFiles = createMemo(() => {
    const byPath = new Map<string, FileDiff>();
    for (const item of props.items) {
      if (item.kind === "tool" && item.diff !== null) {
        byPath.set(item.diff.path, item.diff);
      }
    }
    return [...byPath.values()];
  });

  const workspaceRoot = () => status()?.root_path ?? props.ctx.localPath;

  const statusByPath = createMemo(
    () =>
      new Map<string, WorkspaceGitFileStatus>(
        (status()?.entries ?? []).map((entry) => [entry.absolute_path, entry.status]),
      ),
  );

  /** Load target of the git status (workspace + manual refresh). */
  const statusTarget = createMemo(
    () => ({
      nodeId: props.ctx.nodeId,
      localPath: props.ctx.localPath,
      token: refreshToken(),
    }),
    {
      equals: (left, right) =>
        left.nodeId === right.nodeId &&
        left.localPath === right.localPath &&
        left.token === right.token,
    },
  );

  /** Load target of the branch diff (workspace + scope + manual refresh). */
  const branchTarget = createMemo(
    () => ({
      nodeId: props.ctx.nodeId,
      localPath: props.ctx.localPath,
      scope: scope(),
      token: refreshToken(),
    }),
    {
      equals: (left, right) =>
        left.nodeId === right.nodeId &&
        left.localPath === right.localPath &&
        left.scope === right.scope &&
        left.token === right.token,
    },
  );

  /** Workspace whose data is currently shown. */
  let loadedWorkspace = "";

  createEffect(statusTarget, (target) => {
    const workspace = `${target.nodeId}\u0000${target.localPath}`;
    if (workspace !== loadedWorkspace) {
      loadedWorkspace = workspace;
      setStatus(null);
      setBranchDiff(null);
      setStatusError(null);
      setBranchError(null);
    }
    void loadStatus(target.nodeId, target.localPath);
  });

  createEffect(branchTarget, (target) => {
    if (target.scope !== "branch_base") return;
    void loadBranchDiff(target.nodeId, target.localPath);
  });

  async function loadStatus(nodeId: string, localPath: string): Promise<void> {
    const token = ++statusGeneration;
    setStatusLoading(true);
    setStatusError(null);
    try {
      const next = await sync.connection.client.workspaceGitStatus(nodeId, localPath);
      if (token !== statusGeneration) return;
      setStatus(next);
    } catch (cause) {
      if (token !== statusGeneration) return;
      setStatus(null);
      setStatusError(errorMessage(cause));
    } finally {
      if (token === statusGeneration) setStatusLoading(false);
    }
  }

  async function loadBranchDiff(nodeId: string, localPath: string): Promise<void> {
    const token = ++branchGeneration;
    setBranchLoading(true);
    setBranchError(null);
    try {
      const next = await sync.connection.client.workspaceGitDiff(nodeId, localPath, {
        scope: "branch_base",
      });
      if (token !== branchGeneration) return;
      setBranchDiff(next);
    } catch (cause) {
      if (token !== branchGeneration) return;
      setBranchDiff(null);
      setBranchError(errorMessage(cause));
    } finally {
      if (token === branchGeneration) setBranchLoading(false);
    }
  }

  function refresh(): void {
    setRefreshToken((token) => token + 1);
  }

  /**
   * Git failures carry the useful reason in `message` (rejected push, diverged
   * branch, missing upstream). Prefer it over the generic code label.
   */
  function workspaceError(cause: unknown): string {
    if (cause instanceof ApiError && cause.info !== null) {
      if (cause.info.code === "GIT_FAILED" || cause.info.code === "INVALID_STATE") {
        return cause.info.message;
      }
    }
    return errorMessage(cause);
  }

  async function commit(): Promise<void> {
    if (!canCommit()) return;
    setCommitting(true);
    try {
      const result = await sync.connection.client.workspaceGitCommit(
        props.ctx.nodeId,
        props.ctx.localPath,
        commitMessage().trim(),
      );
      setCommitMessage("");
      toast.success(m.dock_changes_commit_success({ commit: result.commit.slice(0, 8) }));
      refresh();
    } catch (cause) {
      toast.error(workspaceError(cause));
    } finally {
      setCommitting(false);
    }
  }

  async function pull(): Promise<void> {
    if (!canSync()) return;
    setSyncAction("pull");
    try {
      const result = await sync.connection.client.workspaceGitPull(
        props.ctx.nodeId,
        props.ctx.localPath,
      );
      toast.success(
        result.updated
          ? m.dock_changes_pull_updated({ commit: result.commit.slice(0, 8) })
          : m.dock_changes_pull_up_to_date(),
      );
    } catch (cause) {
      toast.error(workspaceError(cause));
    } finally {
      setSyncAction(null);
      refresh();
    }
  }

  async function push(): Promise<void> {
    if (!canSync()) return;
    setSyncAction("push");
    try {
      const result = await sync.connection.client.workspaceGitPush(
        props.ctx.nodeId,
        props.ctx.localPath,
      );
      toast.success(
        result.upstream === null
          ? m.dock_changes_push_success_generic()
          : m.dock_changes_push_success({ upstream: result.upstream }),
      );
    } catch (cause) {
      toast.error(workspaceError(cause));
    } finally {
      setSyncAction(null);
      refresh();
    }
  }

  function openDiff(
    absolutePath: string,
    diffScope: DockDiffScope = "uncommitted",
    preview = true,
  ): void {
    dock.openFile(absolutePath, { mode: "diff", scope: diffScope, preview });
  }

  function sessionAbsolute(file: FileDiff): string {
    return joinPath(props.ctx.localPath, file.path);
  }

  function branchAbsolute(file: FileDiff): string {
    return joinPath(workspaceRoot(), file.path);
  }

  const loadingRow = (label: string) => (
    <div class="text-muted-foreground flex items-center justify-center gap-2 p-4 text-[12px]">
      <IconLoaderCircle class="size-3.5 animate-spin" />
      {label}
    </div>
  );

  return (
    <>
      <div class="border-border flex h-9 shrink-0 items-center gap-1 border-b px-1.5">
        <div class="bg-muted flex min-w-0 items-center rounded-md p-0.5">
          <For each={scopes}>
            {(option) => (
              <button
                type="button"
                class={cn(
                  "truncate rounded-sm px-2 py-0.5 text-[11px] font-medium transition-colors",
                  scope() === option.value
                    ? "bg-card text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                aria-pressed={scope() === option.value ? "true" : "false"}
                onClick={() => setScope(option.value)}
              >
                {option.label}
              </button>
            )}
          </For>
        </div>
        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors"
          title={m.dock_files_refresh()}
          aria-label={m.dock_files_refresh()}
          onClick={refresh}
        >
          <IconRefreshCw class="size-3.5" />
        </button>
        <Show when={scope() === "uncommitted" && status()?.is_repository === true}>
          <span class="text-muted-foreground ml-1 flex shrink-0 items-center gap-1 font-mono text-[11px]">
            <Show when={(status()?.ahead ?? 0) > 0}>
              <span title={m.dock_changes_ahead()}>↑{status()?.ahead}</span>
            </Show>
            <Show when={(status()?.behind ?? 0) > 0}>
              <span title={m.dock_changes_behind()}>↓{status()?.behind}</span>
            </Show>
          </span>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors disabled:opacity-50"
            title={m.dock_changes_pull_hint()}
            aria-label={m.dock_changes_pull()}
            disabled={!canSync()}
            onClick={() => void pull()}
          >
            <Show
              when={syncAction() === "pull"}
              fallback={<IconArrowDownToLine class="size-3.5" />}
            >
              <IconLoaderCircle class="size-3.5 animate-spin" />
            </Show>
          </button>
          <button
            type="button"
            class="text-muted-foreground hover:bg-accent hover:text-foreground flex size-7 shrink-0 items-center justify-center rounded-md transition-colors disabled:opacity-50"
            title={m.dock_changes_push_hint()}
            aria-label={m.dock_changes_push()}
            disabled={!canSync()}
            onClick={() => void push()}
          >
            <Show
              when={syncAction() === "push"}
              fallback={<IconArrowUpFromLine class="size-3.5" />}
            >
              <IconLoaderCircle class="size-3.5 animate-spin" />
            </Show>
          </button>
        </Show>
        <Show when={scope() === "branch_base" && branchDiff() !== null}>
          <span class="text-muted-foreground ml-auto truncate font-mono text-[11px]">
            {branchDiff()?.base_branch ?? branchDiff()?.scope}
            <Show when={(branchDiff()?.head_commit ?? "") !== ""}>
              @ {branchDiff()?.head_commit.slice(0, 8)}
            </Show>
          </span>
        </Show>
      </div>

      <div class="app-scroll min-h-0 flex-1 overflow-y-auto p-1">
        <Show when={branchDiff()?.truncated === true}>
          <p class="text-warning p-2 text-[12px]">{m.diff_truncated()}</p>
        </Show>
        <Switch>
          <Match when={scope() === "session"}>
            <Switch>
              <Match when={sessionFiles().length === 0}>
                <p class="text-muted-foreground p-2 text-[12px]">{m.diff_session_empty()}</p>
              </Match>
              <Match when={sessionFiles().length > 0}>
                <For each={sessionFiles()} keyed={(file) => file.path}>
                  {(file) => {
                    const absolute = () => sessionAbsolute(file());
                    const badge = () => statusByPath().get(absolute());
                    return (
                      <button
                        type="button"
                        class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
                        title={file().path}
                        onClick={() => openDiff(absolute())}
                        onDblClick={() => openDiff(absolute(), "uncommitted", false)}
                      >
                        <Show when={badge()} fallback={<span class="size-4 shrink-0"></span>}>
                          {(current) => <GitStatusBadge status={current()} />}
                        </Show>
                        <span class="min-w-0 flex-1 truncate font-mono">{file().path}</span>
                        <span class="text-ok shrink-0 text-[11px]">+{file().additions}</span>
                        <span class="text-danger shrink-0 text-[11px]">-{file().deletions}</span>
                      </button>
                    );
                  }}
                </For>
              </Match>
            </Switch>
          </Match>

          <Match when={scope() === "uncommitted"}>
            <Switch>
              <Match when={statusLoading() && status() === null}>
                {loadingRow(m.dock_changes_loading())}
              </Match>
              <Match when={statusError() !== null}>
                <p class="text-destructive p-2 text-[12px]">{statusError()}</p>
              </Match>
              <Match when={status() === null}>
                <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_loading()}</p>
              </Match>
              <Match when={status()?.is_repository !== true}>
                <p class="text-muted-foreground p-2 text-[12px]">
                  {m.dock_changes_not_repository()}
                </p>
              </Match>
              <Match when={(status()?.entries.length ?? 0) === 0}>
                <p class="text-muted-foreground p-2 text-[12px]">{m.diff_no_changes()}</p>
              </Match>
              <Match when={(status()?.entries.length ?? 0) > 0}>
                <For each={status()?.entries ?? []} keyed={(entry) => entry.absolute_path}>
                  {(entry) => (
                    <button
                      type="button"
                      class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
                      title={
                        entry().old_path === null
                          ? entry().path
                          : `${entry().old_path} → ${entry().path}`
                      }
                      onClick={() => openDiff(entry().absolute_path)}
                      onDblClick={() => openDiff(entry().absolute_path, "uncommitted", false)}
                    >
                      <GitStatusBadge status={entry().status} />
                      <span class="min-w-0 flex-1 truncate font-mono">{entry().path}</span>
                    </button>
                  )}
                </For>
                <Show when={status()?.truncated === true}>
                  <p class="text-muted-foreground px-1.5 py-1 text-[11px]">
                    {m.dock_files_truncated()}
                  </p>
                </Show>
              </Match>
            </Switch>
          </Match>

          <Match when={branchLoading() && branchDiff() === null}>
            {loadingRow(m.dock_changes_loading())}
          </Match>
          <Match when={branchError() !== null}>
            <p class="text-destructive p-2 text-[12px]">{branchError()}</p>
          </Match>
          <Match when={branchDiff() === null}>
            <p class="text-muted-foreground p-2 text-[12px]">{m.dock_changes_loading()}</p>
          </Match>
          <Match when={branchDiff()?.files.length === 0}>
            <p class="text-muted-foreground p-2 text-[12px]">{m.diff_no_changes()}</p>
          </Match>
          <Match when={(branchDiff()?.files.length ?? 0) > 0}>
            <For each={branchDiff()?.files ?? []} keyed={(file) => file.path}>
              {(file) => {
                const absolute = () => branchAbsolute(file());
                return (
                  <button
                    type="button"
                    class="hover:bg-accent/60 flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left text-[12px]"
                    title={file().path}
                    onClick={() => openDiff(absolute(), "branch_base")}
                    onDblClick={() => openDiff(absolute(), "branch_base", false)}
                  >
                    <span class="min-w-0 flex-1 truncate font-mono">{file().path}</span>
                    <span class="text-ok shrink-0 text-[11px]">+{file().additions}</span>
                    <span class="text-danger shrink-0 text-[11px]">-{file().deletions}</span>
                  </button>
                );
              }}
            </For>
          </Match>
        </Switch>
      </div>

      <Show
        when={
          scope() === "uncommitted" &&
          status()?.is_repository === true &&
          (status()?.entries.length ?? 0) > 0
        }
      >
        <form
          class="border-border flex shrink-0 items-center gap-1.5 border-t p-1.5"
          onSubmit={(event) => {
            event.preventDefault();
            void commit();
          }}
        >
          <input
            class="border-border bg-background focus-visible:ring-ring min-w-0 flex-1 rounded-md border px-2 py-1 text-[12px] outline-none focus-visible:ring-1"
            placeholder={m.dock_changes_commit_placeholder()}
            aria-label={m.dock_changes_commit_placeholder()}
            value={commitMessage()}
            onInput={(event) => setCommitMessage(event.currentTarget.value)}
            disabled={committing()}
          />
          <button
            type="submit"
            class="bg-primary text-primary-foreground hover:bg-primary/90 flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[12px] font-medium transition-colors disabled:opacity-50"
            title={m.dock_changes_commit_hint()}
            disabled={!canCommit()}
          >
            <Show when={committing()}>
              <IconLoaderCircle class="size-3.5 animate-spin" />
            </Show>
            {m.dock_changes_commit_button()}
          </button>
        </form>
      </Show>
    </>
  );
}
