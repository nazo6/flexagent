import { For, Match, Show, Switch, createEffect, createMemo, createSignal } from "solid-js";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import type { FileDiff, WorkspaceDiffResponse } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import { cn } from "#lib/utils.ts";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import DiffViewer from "./DiffViewer.tsx";

type Scope = "session" | "branch_base" | "uncommitted";

export interface DiffPaneProps {
  sessionId: string;
  items: TimelineItem[];
}

/** Session / worktree diff browser with a file list and a preview pane. */
export default function DiffPane(props: DiffPaneProps) {
  const [scope, setScope] = createSignal<Scope>("session");
  const [worktreeDiff, setWorktreeDiff] = createSignal<WorkspaceDiffResponse | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [selectedPath, setSelectedPath] = createSignal<string | null>(null);

  /** "Session changes": aggregate `tool_call` diffs by path (last write wins). */
  const sessionFiles = createMemo(() => {
    const byPath = new Map<string, FileDiff>();
    for (const item of props.items) {
      if (item.kind === "tool" && item.diff !== null) {
        byPath.set(item.diff.path, item.diff);
      }
    }
    return [...byPath.values()];
  });

  const files = createMemo(() =>
    scope() === "session" ? sessionFiles() : (worktreeDiff()?.files ?? []),
  );

  const selected = createMemo(
    () => files().find((file) => file.path === selectedPath()) ?? files().at(0) ?? null,
  );

  const loadWorktreeDiff = async (): Promise<void> => {
    const current = scope();
    if (current === "session") return;
    setLoading(true);
    setError(null);
    try {
      setWorktreeDiff(await sync.connection.client.sessionDiff(props.sessionId, current));
      setSelectedPath(null);
    } catch (cause) {
      setWorktreeDiff(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  createEffect(
    () => scope(),
    (current) => {
      if (current === "session") return;
      void loadWorktreeDiff();
    },
  );

  const scopeOptions: { value: Scope; label: string }[] = [
    { value: "session", label: m.diff_scope_session() },
    { value: "branch_base", label: "Worktree: vs Base" },
    { value: "uncommitted", label: "Worktree: vs HEAD" },
  ];

  return (
    <div class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center gap-1.5">
        <For each={scopeOptions}>
          {(option) => (
            <Button
              size="sm"
              variant={scope() === option.value ? "default" : "outline"}
              onClick={() => setScope(option.value)}
            >
              {option.label}
            </Button>
          )}
        </For>
        <Show when={scope() !== "session"}>
          <Button
            size="sm"
            variant="ghost"
            disabled={loading()}
            onClick={() => void loadWorktreeDiff()}
          >
            <IconRefreshCw />
            {m.diff_refetch()}
          </Button>
        </Show>
        <Show when={worktreeDiff() !== null && scope() !== "session"}>
          <span class="text-muted-foreground ml-auto text-[13px]">
            {worktreeDiff()?.base_branch ?? worktreeDiff()?.scope}
            <Show when={(worktreeDiff()?.head_commit ?? "") !== ""}>
              @ {worktreeDiff()?.head_commit.slice(0, 8)}
            </Show>
          </span>
        </Show>
      </div>

      <Show when={error() !== null}>
        <p class="text-destructive text-sm">{error()}</p>
      </Show>
      <Show when={worktreeDiff()?.truncated === true}>
        <p class="text-warning text-sm">{m.diff_truncated()}</p>
      </Show>

      <Switch>
        <Match when={scope() === "session" && sessionFiles().length === 0}>
          <p class="text-muted-foreground text-sm">{m.diff_session_empty()}</p>
        </Match>
        <Match when={scope() !== "session" && loading()}>
          <p class="text-muted-foreground text-sm">{m.diff_loading()}</p>
        </Match>
        <Match when={files().length === 0 && error() === null}>
          <p class="text-muted-foreground text-sm">{m.diff_no_changes()}</p>
        </Match>
        <Match when={files().length > 0}>
          <div class="grid gap-3 lg:grid-cols-[minmax(0,20rem)_minmax(0,1fr)]">
            <ul class="flex max-h-[65svh] list-none flex-col gap-0.5 overflow-y-auto p-0">
              <For each={files()}>
                {(file) => (
                  <li>
                    <button
                      type="button"
                      class={cn(
                        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px]",
                        selected()?.path === file.path
                          ? "bg-accent text-accent-foreground"
                          : "hover:bg-muted",
                      )}
                      onClick={() => setSelectedPath(file.path)}
                    >
                      <span class="truncate font-mono">{file.path}</span>
                      <span class="ml-auto flex shrink-0 items-center gap-1">
                        <Show when={file.additions > 0}>
                          <Badge variant="secondary" class="text-ok">
                            +{file.additions}
                          </Badge>
                        </Show>
                        <Show when={file.deletions > 0}>
                          <Badge variant="secondary" class="text-danger">
                            -{file.deletions}
                          </Badge>
                        </Show>
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
            <div class="min-w-0">
              <DiffViewer file={selected()} />
            </div>
          </div>
        </Match>
      </Switch>
    </div>
  );
}
