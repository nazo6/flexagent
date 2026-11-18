import { For, Show, createMemo, createSignal } from "solid-js";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { formatRelativeTime } from "#lib/format.ts";
import type { ProjectSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import type { ResourceEntry } from "#lib/stores/resource.ts";
import IconEraser from "~icons/lucide/eraser";
import IconGitBranch from "~icons/lucide/git-branch";
import IconPlay from "~icons/lucide/play";
import IconPlus from "~icons/lucide/plus";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconTrash2 from "~icons/lucide/trash-2";

/**
 * One logical project: its folder bindings per node, the worktrees loaded on
 * demand, and the actions offered by the projects page.
 *
 * The worktree list comes in as a resource entry so the page owns fetching and
 * error reporting; this card only renders the state it is given.
 */
export interface ProjectCardProps {
  project: ProjectSummary;
  worktreeState: ResourceEntry<WorktreeInfo[]>;
  nodeName: (nodeId: string) => string;
  onRefreshWorktrees: (projectId: string) => void;
  onCreateWorktree: (projectId: string) => void;
  onPrune: (projectId: string) => void;
  onNewSession: (projectId: string, nodeId?: string, path?: string) => void;
  onRemoveWorktree: (projectId: string, worktree: WorktreeInfo) => void;
  onUnlinkBinding: (projectId: string, nodeId: string, path: string) => void;
}

export function ProjectCard(props: ProjectCardProps) {
  /** Binding awaiting removal confirmation. */
  const [unlinkTarget, setUnlinkTarget] = createSignal<{ nodeId: string; path: string } | null>(
    null,
  );

  const worktrees = createMemo(() => props.worktreeState.data ?? []);

  return (
    <section class="border-border flex flex-col gap-3 border-b py-4">
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div class="flex min-w-0 flex-col gap-1">
          <h2 class="truncate text-sm font-semibold tracking-tight">{props.project.name}</h2>
          <p class="text-muted-foreground desk-mono truncate text-[11px]">
            {props.project.project_id}
          </p>
          <Show when={props.project.canonical_git_url}>
            <p class="text-muted-foreground truncate text-[11px]">
              {props.project.canonical_git_url}
            </p>
          </Show>
          <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
            <Badge variant="outline" class="h-5 text-[11px]">
              {props.project.identity.kind === "git_remote"
                ? "Git"
                : props.project.identity.kind === "user_key"
                  ? "User key"
                  : "Local only"}
            </Badge>
            <Show when={props.project.identity.kind === "local"}>
              <span class="text-muted-foreground text-[11px]">
                This machine only. Set a project key to share it across machines.
              </span>
            </Show>
          </div>
        </div>
        <div class="flex flex-wrap items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-[11px]"
            disabled={props.worktreeState.loading}
            onClick={() => props.onRefreshWorktrees(props.project.project_id)}
          >
            <IconRefreshCw class="size-3.5" />
            {m.projects_worktrees()}
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-[11px]"
            onClick={() => props.onCreateWorktree(props.project.project_id)}
          >
            <IconPlus class="size-3.5" />
            {m.projects_worktree_add()}
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-[11px]"
            onClick={() => props.onPrune(props.project.project_id)}
            title={m.projects_prune_title()}
          >
            <IconEraser class="size-3.5" />
            Prune
          </Button>
          <Button
            size="sm"
            class="h-7 text-[11px]"
            onClick={() => {
              props.onNewSession(props.project.project_id);
            }}
          >
            <IconPlay class="size-3.5" />
            {m.projects_new_session()}
          </Button>
        </div>
      </div>

      <Show when={props.project.bindings.length > 0}>
        <ul class="border-border ml-1 flex list-none flex-col gap-1 border-l p-0 pl-3 text-[11px]">
          <For each={props.project.bindings}>
            {(binding) => (
              <li class="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{props.nodeName(binding.node_id)}</Badge>
                <span
                  class={
                    binding.path_exists
                      ? "font-mono"
                      : "font-mono text-muted-foreground line-through"
                  }
                  title={
                    binding.path_exists
                      ? binding.local_path
                      : m.projects_path_missing_title({ path: binding.local_path })
                  }
                >
                  {binding.local_path}
                </span>
                <Show when={!binding.path_exists}>
                  <Badge variant="outline" class="border-destructive/50 text-destructive">
                    {m.projects_path_missing()}
                  </Badge>
                </Show>
                <Show when={binding.git_branch}>
                  <span class="text-muted-foreground">({binding.git_branch})</span>
                </Show>
                <Show when={binding.is_worktree}>
                  <Badge variant="secondary">worktree</Badge>
                </Show>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-foreground"
                  onClick={() =>
                    props.onNewSession(
                      props.project.project_id,
                      binding.node_id,
                      binding.local_path,
                    )
                  }
                  title={m.projects_new_session_here()}
                >
                  <IconPlay class="size-3.5" />
                  <span class="sr-only">{m.projects_new_session_here()}</span>
                </Button>
                <span class="text-muted-foreground ml-auto">
                  {formatRelativeTime(binding.last_used_at)}
                </span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-destructive hover:bg-destructive/10"
                  onClick={() =>
                    setUnlinkTarget({ nodeId: binding.node_id, path: binding.local_path })
                  }
                  title={m.projects_unlink_tooltip()}
                >
                  <IconTrash2 class="size-3.5" />
                  <span class="sr-only">{m.projects_unlink_tooltip()}</span>
                </Button>
              </li>
            )}
          </For>
        </ul>
      </Show>

      <Show when={props.worktreeState.loading && worktrees().length === 0}>
        <p class="text-muted-foreground text-[11px]">{m.common_loading()}</p>
      </Show>
      <Show when={props.worktreeState.error !== null && worktrees().length === 0}>
        <div class="flex items-center gap-2 text-[11px]">
          <p class="text-destructive">
            {m.common_load_failed()} {props.worktreeState.error}
          </p>
          <Button
            variant="ghost"
            size="xs"
            onClick={() => props.onRefreshWorktrees(props.project.project_id)}
          >
            {m.common_retry()}
          </Button>
        </div>
      </Show>
      <Show when={worktrees().length > 0}>
        <ul class="border-border ml-1 flex list-none flex-col gap-1 border-l p-0 pl-3 text-[11px]">
          <For each={worktrees()}>
            {(worktree) => (
              <li class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md px-2 py-1.5">
                <IconGitBranch class="text-muted-foreground size-3.5" />
                <span class="font-mono">{worktree.branch ?? "(detached)"}</span>
                <span class="text-muted-foreground truncate">{worktree.path}</span>
                <Badge variant="outline">{props.nodeName(worktree.node_id)}</Badge>
                <Show when={worktree.is_main}>
                  <Badge variant="secondary">main</Badge>
                </Show>
                <Show when={worktree.head_commit}>
                  <span class="text-muted-foreground font-mono">
                    {worktree.head_commit?.slice(0, 8)}
                  </span>
                </Show>
                <Show when={!worktree.is_main}>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="text-muted-foreground hover:text-destructive ml-auto"
                    onClick={() => props.onRemoveWorktree(props.project.project_id, worktree)}
                  >
                    <IconTrash2 />
                    <span class="sr-only">{m.projects_worktree_delete()}</span>
                  </Button>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>

      <AlertDialog.Root
        open={unlinkTarget() !== null}
        onOpenChange={(next) => {
          if (!next) setUnlinkTarget(null);
        }}
      >
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title>{m.projects_unlink_title()}</AlertDialog.Title>
            <AlertDialog.Description>
              {m.projects_unlink_description({
                path: unlinkTarget()?.path ?? "",
                node: props.nodeName(unlinkTarget()?.nodeId ?? ""),
              })}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action
              variant="destructive"
              onClick={() => {
                const target = unlinkTarget();
                if (target !== null) {
                  props.onUnlinkBinding(props.project.project_id, target.nodeId, target.path);
                }
                setUnlinkTarget(null);
              }}
            >
              {m.projects_unlink_action()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </section>
  );
}
