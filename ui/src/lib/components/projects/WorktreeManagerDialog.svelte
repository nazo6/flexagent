<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { NodeSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import EraserIcon from "@lucide/svelte/icons/eraser";
  import { toast } from "svelte-sonner";

  interface Props {
    nodes: NodeSummary[];
    nodeName: (nodeId: string) => string;
    /** Called after a worktree was created, removed or pruned. */
    onChanged: (projectId: string) => void;
  }

  let { nodes, nodeName, onChanged }: Props = $props();

  // Create worktree dialog
  let createProjectId = $state<string | null>(null);
  let createNodeId = $state("");
  let createBranch = $state("");
  let createBase = $state("");
  let createBusy = $state(false);

  // Worktree removal confirmation
  let removeTarget = $state<{
    projectId: string;
    worktree: WorktreeInfo;
  } | null>(null);
  let removeBusy = $state(false);

  // Worktree cleanup (prune) confirmation
  let pruneProjectId = $state<string | null>(null);
  let pruneNodeId = $state("");
  let pruneBusy = $state(false);

  /** Default online node selection (the first one). */
  function defaultOnlineNode(): string {
    return nodes.find((node) => node.is_online)?.node_id ?? "";
  }

  /** Opens the create-worktree dialog for a project. */
  export function openCreate(projectId: string): void {
    createProjectId = projectId;
    createNodeId = defaultOnlineNode();
    createBranch = "";
    createBase = "";
  }

  /** Opens the removal confirmation for a worktree. */
  export function openRemove(projectId: string, worktree: WorktreeInfo): void {
    removeTarget = { projectId, worktree };
  }

  /** Prefer an online node bound to this project when choosing the prune target. */
  export function openPrune(projectId: string): void {
    pruneProjectId = projectId;
    const project = sync.projects.find((entry) => entry.project_id === projectId);
    const boundOnline = project?.bindings.find((binding) =>
      nodes.some((node) => node.node_id === binding.node_id && node.is_online),
    );
    pruneNodeId = boundOnline?.node_id ?? defaultOnlineNode();
  }

  async function submitCreateWorktree(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const projectId = createProjectId;
    if (projectId === null || createBusy) return;
    createBusy = true;
    try {
      const created = await sync.connection.client.createWorktree(projectId, {
        node_id: createNodeId,
        branch: createBranch.trim(),
        base_branch: createBase.trim() === "" ? null : createBase.trim(),
      });
      toast.success(m.projects_worktree_created({ path: created.path }));
      createProjectId = null;
      onChanged(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      createBusy = false;
    }
  }

  async function confirmRemove(force: boolean): Promise<void> {
    const target = removeTarget;
    if (target === null || removeBusy) return;
    removeBusy = true;
    try {
      await sync.connection.client.removeWorktree(target.projectId, {
        node_id: target.worktree.node_id,
        path: target.worktree.path,
        force,
      });
      toast.success(m.projects_worktree_removed());
      removeTarget = null;
      onChanged(target.projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      removeBusy = false;
    }
  }

  async function confirmPrune(): Promise<void> {
    const projectId = pruneProjectId;
    if (projectId === null || pruneBusy || pruneNodeId === "") return;
    pruneBusy = true;
    try {
      await sync.connection.client.pruneWorktrees(projectId, {
        node_id: pruneNodeId,
      });
      toast.success(m.projects_prune_success());
      pruneProjectId = null;
      onChanged(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pruneBusy = false;
    }
  }
</script>

<!-- Create worktree -->
<Dialog.Root
  open={createProjectId !== null}
  onOpenChange={(open) => !open && (createProjectId = null)}
>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.projects_create_worktree_title()}</Dialog.Title>
      <Dialog.Description>
        <code>git worktree add -b &lt;branch&gt;</code>
        {m.projects_create_worktree_description()}
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitCreateWorktree}>
      <div class="grid gap-2">
        <Label for="wt-node">{m.projects_node()}</Label>
        <select
          id="wt-node"
          bind:value={createNodeId}
          class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
        >
          {#each nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}{node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
            </option>
          {/each}
        </select>
      </div>
      <div class="grid gap-2">
        <Label for="wt-branch">{m.projects_branch_name()}</Label>
        <Input id="wt-branch" bind:value={createBranch} placeholder="feat/new-feature" />
      </div>
      <div class="grid gap-2">
        <Label for="wt-base">{m.projects_base_branch()}</Label>
        <Input id="wt-base" bind:value={createBase} placeholder="main" />
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (createProjectId = null)}>
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={createBusy || createBranch.trim() === ""}>
          {createBusy ? m.projects_creating() : m.projects_create()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- Worktree removal confirmation -->
<AlertDialog.Root
  open={removeTarget !== null}
  onOpenChange={(open) => !open && (removeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.projects_delete_worktree_title()}</AlertDialog.Title>
      <AlertDialog.Description>
        {m.projects_delete_worktree_description({
          path: removeTarget?.worktree.path ?? "",
          node: nodeName(removeTarget?.worktree.node_id ?? ""),
        })}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={removeBusy}
        onclick={() => void confirmRemove(false)}
      >
        {m.common_delete()}
      </AlertDialog.Action>
      <AlertDialog.Action
        variant="destructive"
        disabled={removeBusy}
        onclick={() => void confirmRemove(true)}
      >
        {m.projects_force_delete()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- Worktree cleanup (prune) confirmation -->
<AlertDialog.Root
  open={pruneProjectId !== null}
  onOpenChange={(open) => !open && (pruneProjectId = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title class="flex items-center gap-2">
        <EraserIcon class="size-5" />
        {m.projects_prune_confirm_title()}
      </AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        <code>git worktree prune</code>
        {m.projects_prune_description()}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <div class="grid gap-2">
      <Label for="prune-node">{m.projects_run_node()}</Label>
      <select
        id="prune-node"
        bind:value={pruneNodeId}
        class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
      >
        {#each nodes as node (node.node_id)}
          <option value={node.node_id} disabled={!node.is_online}>
            {node.name}{node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
          </option>
        {/each}
      </select>
    </div>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={pruneBusy}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        disabled={pruneBusy || pruneNodeId === ""}
        onclick={() => void confirmPrune()}
      >
        {pruneBusy ? m.projects_prune_running() : m.projects_prune_execute()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
