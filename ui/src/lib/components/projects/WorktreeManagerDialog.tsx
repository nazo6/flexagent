import type { JSX } from "@solidjs/web";
import { For, createSignal } from "solid-js";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import type { NodeSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import IconEraser from "~icons/lucide/eraser";
import { toast } from "#lib/components/ui/sonner/index.ts";

export interface WorktreeManagerDialogOptions {
  /** Nodes a worktree can be created or pruned on (the page's `sync.nodes`). */
  nodes: () => NodeSummary[];
  nodeName: (nodeId: string) => string;
  /** Called after a worktree was created, removed or pruned. */
  onChanged: (projectId: string) => void;
}

export interface WorktreeManagerDialogHandle {
  /** Opens the create-worktree dialog for a project. */
  openCreate: (projectId: string) => void;
  /** Opens the removal confirmation for a worktree. */
  openRemove: (projectId: string, worktree: WorktreeInfo) => void;
  /** Opens the prune confirmation for a project. */
  openPrune: (projectId: string) => void;
  /** The dialogs; render them once inside the page. */
  View: () => JSX.Element;
}

/**
 * Worktree create / remove / prune dialogs, ported from
 * `WorktreeManagerDialog.svelte`.
 *
 * Three independent dialogs share one handle, as they did behind one
 * `bind:this` in the Svelte page. Removal intentionally does not close the
 * dialog by itself: the busy state stays visible until the node answers.
 */
export function createWorktreeManagerDialog(
  options: WorktreeManagerDialogOptions,
): WorktreeManagerDialogHandle {
  // Create worktree dialog
  const [createProjectId, setCreateProjectId] = createSignal<string | null>(null);
  const [createNodeId, setCreateNodeId] = createSignal("");
  const [createBranch, setCreateBranch] = createSignal("");
  const [createBase, setCreateBase] = createSignal("");
  const [createBusy, setCreateBusy] = createSignal(false);

  // Worktree removal confirmation
  const [removeTarget, setRemoveTarget] = createSignal<{
    projectId: string;
    worktree: WorktreeInfo;
  } | null>(null);
  const [removeBusy, setRemoveBusy] = createSignal(false);

  // Worktree cleanup (prune) confirmation
  const [pruneProjectId, setPruneProjectId] = createSignal<string | null>(null);
  const [pruneNodeId, setPruneNodeId] = createSignal("");
  const [pruneBusy, setPruneBusy] = createSignal(false);

  /** Default online node selection (the first one). */
  function defaultOnlineNode(): string {
    return options.nodes().find((node) => node.is_online)?.node_id ?? "";
  }

  function openCreate(projectId: string): void {
    setCreateProjectId(projectId);
    setCreateNodeId(defaultOnlineNode());
    setCreateBranch("");
    setCreateBase("");
  }

  function openRemove(projectId: string, worktree: WorktreeInfo): void {
    setRemoveTarget({ projectId, worktree });
  }

  /** Prefer an online node bound to this project when choosing the prune target. */
  function openPrune(projectId: string): void {
    setPruneProjectId(projectId);
    const project = sync.projects.find((entry) => entry.project_id === projectId);
    const boundOnline = project?.bindings.find((binding) =>
      options.nodes().some((node) => node.node_id === binding.node_id && node.is_online),
    );
    setPruneNodeId(boundOnline?.node_id ?? defaultOnlineNode());
  }

  async function submitCreateWorktree(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const projectId = createProjectId();
    const nodeId = createNodeId();
    if (projectId === null || createBusy() || nodeId === "") return;
    setCreateBusy(true);
    try {
      const created = await sync.connection.client.createWorktree(projectId, {
        node_id: nodeId,
        branch: createBranch().trim(),
        base_branch: createBase().trim() === "" ? null : createBase().trim(),
      });
      toast.success(m.projects_worktree_created({ path: created.path }));
      setCreateProjectId(null);
      options.onChanged(projectId);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setCreateBusy(false);
    }
  }

  async function confirmRemove(force: boolean): Promise<void> {
    const target = removeTarget();
    if (target === null || removeBusy()) return;
    setRemoveBusy(true);
    try {
      await sync.connection.client.removeWorktree(target.projectId, {
        node_id: target.worktree.node_id,
        path: target.worktree.path,
        force,
      });
      toast.success(m.projects_worktree_removed());
      setRemoveTarget(null);
      options.onChanged(target.projectId);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setRemoveBusy(false);
    }
  }

  async function confirmPrune(): Promise<void> {
    const projectId = pruneProjectId();
    if (projectId === null || pruneBusy() || pruneNodeId() === "") return;
    setPruneBusy(true);
    try {
      await sync.connection.client.pruneWorktrees(projectId, {
        node_id: pruneNodeId(),
      });
      toast.success(m.projects_prune_success());
      setPruneProjectId(null);
      options.onChanged(projectId);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setPruneBusy(false);
    }
  }

  function NodeOptions() {
    return (
      <>
        <option value="">
          {options.nodes().some((node) => node.is_online)
            ? m.settings_select_node()
            : m.common_no_node_online()}
        </option>
        <For each={options.nodes()}>
          {(node) => (
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}
              {node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
            </option>
          )}
        </For>
      </>
    );
  }

  function View() {
    return (
      <>
        {/* Create worktree */}
        <Dialog.Root
          open={createProjectId() !== null}
          onOpenChange={(open) => {
            if (!open) setCreateProjectId(null);
          }}
        >
          <Dialog.Content class="sm:max-w-md">
            <Dialog.Header>
              <Dialog.Title>{m.projects_create_worktree_title()}</Dialog.Title>
              <Dialog.Description>
                <code>git worktree add -b &lt;branch&gt;</code>{" "}
                {m.projects_create_worktree_description()}
              </Dialog.Description>
            </Dialog.Header>
            <form class="flex flex-col gap-4" onSubmit={submitCreateWorktree}>
              <div class="grid gap-2">
                <Label for="wt-node">{m.projects_node()}</Label>
                <select
                  id="wt-node"
                  value={createNodeId()}
                  onChange={(event) => setCreateNodeId(event.currentTarget.value)}
                  class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
                >
                  <NodeOptions />
                </select>
              </div>
              <div class="grid gap-2">
                <Label for="wt-branch">{m.projects_branch_name()}</Label>
                <Input
                  id="wt-branch"
                  value={createBranch()}
                  onValueChange={setCreateBranch}
                  placeholder="feat/new-feature"
                />
              </div>
              <div class="grid gap-2">
                <Label for="wt-base">{m.projects_base_branch()}</Label>
                <Input
                  id="wt-base"
                  value={createBase()}
                  onValueChange={setCreateBase}
                  placeholder="main"
                />
              </div>
              <Dialog.Footer>
                <Button type="button" variant="outline" onClick={() => setCreateProjectId(null)}>
                  {m.common_cancel()}
                </Button>
                <Button
                  type="submit"
                  disabled={createBusy() || createNodeId() === "" || createBranch().trim() === ""}
                >
                  {createBusy() ? m.projects_creating() : m.projects_create()}
                </Button>
              </Dialog.Footer>
            </form>
          </Dialog.Content>
        </Dialog.Root>

        {/* Worktree removal confirmation */}
        <AlertDialog.Root
          open={removeTarget() !== null}
          onOpenChange={(open) => {
            if (!open) setRemoveTarget(null);
          }}
        >
          <AlertDialog.Content>
            <AlertDialog.Header>
              <AlertDialog.Title>{m.projects_delete_worktree_title()}</AlertDialog.Title>
              <AlertDialog.Description>
                {m.projects_delete_worktree_description({
                  path: removeTarget()?.worktree.path ?? "",
                  node: options.nodeName(removeTarget()?.worktree.node_id ?? ""),
                })}
              </AlertDialog.Description>
            </AlertDialog.Header>
            <AlertDialog.Footer>
              <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
              <AlertDialog.Action
                variant="destructive"
                disabled={removeBusy()}
                onClick={() => void confirmRemove(false)}
              >
                {m.common_delete()}
              </AlertDialog.Action>
              <AlertDialog.Action
                variant="destructive"
                disabled={removeBusy()}
                onClick={() => void confirmRemove(true)}
              >
                {m.projects_force_delete()}
              </AlertDialog.Action>
            </AlertDialog.Footer>
          </AlertDialog.Content>
        </AlertDialog.Root>

        {/* Worktree cleanup (prune) confirmation */}
        <AlertDialog.Root
          open={pruneProjectId() !== null}
          onOpenChange={(open) => {
            if (!open) setPruneProjectId(null);
          }}
        >
          <AlertDialog.Content>
            <AlertDialog.Header>
              <AlertDialog.Title class="flex items-center gap-2">
                <IconEraser class="size-5" />
                {m.projects_prune_confirm_title()}
              </AlertDialog.Title>
              <AlertDialog.Description class="text-sm">
                <code>git worktree prune</code> {m.projects_prune_description()}
              </AlertDialog.Description>
            </AlertDialog.Header>
            <div class="grid gap-2">
              <Label for="prune-node">{m.projects_run_node()}</Label>
              <select
                id="prune-node"
                value={pruneNodeId()}
                onChange={(event) => setPruneNodeId(event.currentTarget.value)}
                class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
              >
                <NodeOptions />
              </select>
            </div>
            <AlertDialog.Footer>
              <AlertDialog.Cancel disabled={pruneBusy()}>{m.common_cancel()}</AlertDialog.Cancel>
              <AlertDialog.Action
                disabled={pruneBusy() || pruneNodeId() === ""}
                onClick={() => void confirmPrune()}
              >
                {pruneBusy() ? m.projects_prune_running() : m.projects_prune_execute()}
              </AlertDialog.Action>
            </AlertDialog.Footer>
          </AlertDialog.Content>
        </AlertDialog.Root>
      </>
    );
  }

  return { openCreate, openRemove, openPrune, View };
}
