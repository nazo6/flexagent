<script lang="ts">
  import { goto } from "$app/navigation";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import type { ProjectSummary, WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { shortId } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import EraserIcon from "@lucide/svelte/icons/eraser";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import LinkIcon from "@lucide/svelte/icons/link";
  import PlayIcon from "@lucide/svelte/icons/play";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let worktreesByProject = $state(new Map<string, WorktreeInfo[]>());
  let loadingProjects = $state<string[]>([]);

  // Create worktree dialog
  let createProjectId = $state<string | null>(null);
  let createNodeId = $state("");
  let createBranch = $state("");
  let createBase = $state("");
  let createPath = $state("");
  let createBusy = $state(false);
  let browserOpen = $state(false);
  /** Which field receives the folder browser selection. */
  let browserTarget = $state<"create" | "scan" | "link">("create");

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

  // Bulk project scan
  let scanOpen = $state(false);
  let scanNodeId = $state("");
  let scanDir = $state("");
  let scanBusy = $state(false);

  // Manual folder linking
  let linkOpen = $state(false);
  let linkNodeId = $state("");
  let linkProjectId = $state("");
  let linkPath = $state("");
  let linkBusy = $state(false);

  /** Node the folder browser browses on. */
  const browserNodeId = $derived(
    browserTarget === "create" ? createNodeId : browserTarget === "scan" ? scanNodeId : linkNodeId,
  );

  /** Initial path for the folder browser. */
  const browserInitialPath = $derived(
    browserTarget === "create" ? createPath : browserTarget === "scan" ? scanDir : linkPath,
  );

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }

  async function loadWorktrees(projectId: string, silent = false): Promise<void> {
    if (loadingProjects.includes(projectId)) return;
    loadingProjects = [...loadingProjects, projectId];
    try {
      const list = await sync.connection.client.worktrees(projectId);
      worktreesByProject = new Map(worktreesByProject).set(projectId, list);
    } catch (error) {
      if (!silent) toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loadingProjects = loadingProjects.filter((id) => id !== projectId);
    }
  }

  function openCreateWorktree(projectId: string) {
    createProjectId = projectId;
    createNodeId = sync.nodes.find((node) => node.is_online)?.node_id ?? "";
    createBranch = "";
    createBase = "";
    createPath = "";
  }

  /** Open the new session screen for this project (optionally scoped to a node and path). */
  function openNewSession(projectId: string, bindingNodeId?: string, path?: string) {
    const params = new URLSearchParams({ project: projectId });
    if (bindingNodeId) params.set("node", bindingNodeId);
    if (path) params.set("path", path);
    void goto(`/?${params.toString()}`);
  }

  /** Default online node selection (the first one). */
  function defaultOnlineNode(): string {
    return sync.nodes.find((node) => node.is_online)?.node_id ?? "";
  }

  function openScanDialog() {
    scanNodeId = defaultOnlineNode();
    scanDir = "";
    scanOpen = true;
  }

  async function submitScan(event: SubmitEvent) {
    event.preventDefault();
    if (scanBusy || scanNodeId === "") return;
    scanBusy = true;
    try {
      const response = await sync.connection.client.scanProjects(scanNodeId, {
        dir: scanDir.trim() === "" ? null : scanDir.trim(),
      });
      toast.success(
        m.projects_scan_complete({
          dirs: response.scanned_dirs.length,
          projects: response.projects.length,
        }),
      );
      scanOpen = false;
      await sync.refreshProjects();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      scanBusy = false;
    }
  }

  function openLinkDialog() {
    linkNodeId = defaultOnlineNode();
    linkProjectId = "";
    linkPath = "";
    linkOpen = true;
  }

  async function submitLink(event: SubmitEvent) {
    event.preventDefault();
    if (linkBusy || linkNodeId === "" || linkProjectId.trim() === "" || linkPath.trim() === "") {
      return;
    }
    linkBusy = true;
    try {
      const result = await sync.connection.client.linkProject(linkNodeId, {
        project_id: linkProjectId.trim(),
        local_path: linkPath.trim(),
      });
      toast.success(
        m.projects_link_success({ path: result.local_path, project: result.project_id }),
      );
      linkOpen = false;
      await sync.refreshProjects();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      linkBusy = false;
    }
  }

  /** Prefer an online node bound to this project when choosing the prune target. */
  function openPruneDialog(projectId: string) {
    pruneProjectId = projectId;
    const project = sync.projects.find((entry) => entry.project_id === projectId);
    const boundOnline = project?.bindings.find((binding) =>
      sync.nodes.some((node) => node.node_id === binding.node_id && node.is_online),
    );
    pruneNodeId = boundOnline?.node_id ?? defaultOnlineNode();
  }

  async function confirmPrune() {
    const projectId = pruneProjectId;
    if (projectId === null || pruneBusy || pruneNodeId === "") return;
    pruneBusy = true;
    try {
      await sync.connection.client.pruneWorktrees(projectId, {
        node_id: pruneNodeId,
      });
      toast.success(m.projects_prune_success());
      pruneProjectId = null;
      await loadWorktrees(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pruneBusy = false;
    }
  }

  async function submitCreateWorktree(event: SubmitEvent) {
    event.preventDefault();
    const projectId = createProjectId;
    if (projectId === null || createBusy) return;
    createBusy = true;
    try {
      const created = await sync.connection.client.createWorktree(projectId, {
        node_id: createNodeId,
        branch: createBranch.trim(),
        base_branch: createBase.trim() === "" ? null : createBase.trim(),
        path: createPath.trim() === "" ? null : createPath.trim(),
      });
      toast.success(m.projects_worktree_created({ path: created.path }));
      createProjectId = null;
      await loadWorktrees(projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      createBusy = false;
    }
  }

  async function confirmRemove(force: boolean) {
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
      await loadWorktrees(target.projectId);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      removeBusy = false;
    }
  }
</script>

<div class="mx-auto flex max-w-5xl flex-col gap-4 p-4 md:p-6">
  <div class="flex items-center justify-between gap-2">
    <div>
      <h1 class="text-xl font-semibold">{m.projects_title()}</h1>
      <p class="text-muted-foreground text-sm">{m.projects_subtitle()}</p>
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button variant="outline" size="sm" onclick={openScanDialog} title={m.projects_scan_title()}>
        <SearchIcon />
        {m.projects_scan()}
      </Button>
      <Button variant="outline" size="sm" onclick={openLinkDialog} title={m.projects_link_title()}>
        <LinkIcon />
        {m.projects_link()}
      </Button>
      <Button variant="outline" size="sm" onclick={() => void sync.refreshProjects()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
    </div>
  </div>

  {#if sync.projects.length === 0}
    <p class="text-muted-foreground text-sm">{m.projects_empty()}</p>
  {:else}
    {#each sync.projects as project (project.project_id)}
      <Card class="gap-3 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="flex min-w-0 flex-col gap-0.5">
            <h2 class="truncate font-medium">{project.name}</h2>
            <p class="text-muted-foreground truncate text-xs">
              {project.project_id}
            </p>
            {#if project.canonical_git_url}
              <p class="text-muted-foreground truncate text-xs">
                {project.canonical_git_url}
              </p>
            {/if}
          </div>
          <div class="flex flex-wrap items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              disabled={loadingProjects.includes(project.project_id)}
              onclick={() => void loadWorktrees(project.project_id)}
            >
              <RefreshCwIcon />
              Worktree
            </Button>
            <Button
              variant="outline"
              size="sm"
              onclick={() => openCreateWorktree(project.project_id)}
            >
              <PlusIcon />
              {m.projects_worktree_add()}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onclick={() => openPruneDialog(project.project_id)}
              title={m.projects_prune_title()}
            >
              <EraserIcon />
              Prune
            </Button>
            <Button
              size="sm"
              onclick={() => {
                openNewSession(project.project_id);
              }}
            >
              <PlayIcon />
              {m.projects_new_session()}
            </Button>
          </div>
        </div>

        {#if project.bindings.length > 0}
          <ul class="flex list-none flex-col gap-1 p-0 text-xs">
            {#each project.bindings as binding (`${binding.node_id}|${binding.local_path}`)}
              <li class="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{nodeName(binding.node_id)}</Badge>
                <span
                  class="font-mono {binding.path_exists
                    ? ''
                    : 'text-muted-foreground line-through'}"
                  title={binding.path_exists
                    ? binding.local_path
                    : m.projects_path_missing_title({ path: binding.local_path })}
                >
                  {binding.local_path}
                </span>
                {#if !binding.path_exists}
                  <Badge variant="outline" class="border-destructive/50 text-destructive">
                    {m.projects_path_missing()}
                  </Badge>
                {/if}
                {#if binding.git_branch}
                  <span class="text-muted-foreground">({binding.git_branch})</span>
                {/if}
                {#if binding.is_worktree}
                  <Badge variant="secondary">worktree</Badge>
                {/if}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-foreground"
                  onclick={() =>
                    openNewSession(project.project_id, binding.node_id, binding.local_path)}
                  title={m.projects_new_session_here()}
                >
                  <PlayIcon class="size-3.5" />
                  <span class="sr-only">{m.projects_new_session_here()}</span>
                </Button>
                <span class="text-muted-foreground ml-auto">
                  {formatRelativeTime(binding.last_used_at)}
                </span>
              </li>
            {/each}
          </ul>
        {/if}

        {@const worktrees = worktreesByProject.get(project.project_id) ?? []}
        {#if worktrees.length > 0}
          <ul class="flex list-none flex-col gap-1 p-0 text-xs">
            {#each worktrees as worktree (`${worktree.node_id}|${worktree.path}`)}
              <li
                class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md px-2 py-1.5"
              >
                <GitBranchIcon class="text-muted-foreground size-3.5" />
                <span class="font-mono">{worktree.branch ?? "(detached)"}</span>
                <span class="text-muted-foreground truncate">{worktree.path}</span>
                <Badge variant="outline">{nodeName(worktree.node_id)}</Badge>
                {#if worktree.is_main}
                  <Badge variant="secondary">main</Badge>
                {/if}
                {#if worktree.head_commit}
                  <span class="text-muted-foreground font-mono">
                    {worktree.head_commit.slice(0, 8)}
                  </span>
                {/if}
                {#if !worktree.is_main}
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    class="text-muted-foreground hover:text-destructive ml-auto"
                    onclick={() =>
                      (removeTarget = {
                        projectId: project.project_id,
                        worktree,
                      })}
                  >
                    <Trash2Icon />
                    <span class="sr-only">{m.projects_worktree_delete()}</span>
                  </Button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </Card>
    {/each}
  {/if}
</div>

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
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
        >
          {#each sync.nodes as node (node.node_id)}
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
      <div class="grid gap-2">
        <Label for="wt-path">{m.projects_target_path()}</Label>
        <div class="flex items-center gap-1.5">
          <Input
            id="wt-path"
            bind:value={createPath}
            placeholder={m.projects_path_auto()}
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!createNodeId}
            onclick={() => {
              browserTarget = "create";
              browserOpen = true;
            }}
            title={m.projects_pick_folder()}
          >
            <FolderOpenIcon class="size-3.5" />
            <span>{m.projects_browse()}</span>
          </Button>
        </div>
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
        class="border-input bg-background h-9 rounded-md border px-2 text-sm"
      >
        {#each sync.nodes as node (node.node_id)}
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

<!-- Bulk project scan -->
<Dialog.Root open={scanOpen} onOpenChange={(open) => (scanOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.projects_scan_dialog_title()}</Dialog.Title>
      <Dialog.Description>
        {m.projects_scan_dialog_description()}
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitScan}>
      <div class="grid gap-2">
        <Label for="scan-node">{m.projects_node()}</Label>
        <select
          id="scan-node"
          bind:value={scanNodeId}
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
        >
          {#each sync.nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}{node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
            </option>
          {/each}
        </select>
      </div>
      <div class="grid gap-2">
        <Label for="scan-dir">{m.projects_scan_dir()}</Label>
        <div class="flex items-center gap-1.5">
          <Input
            id="scan-dir"
            bind:value={scanDir}
            placeholder={m.projects_scan_dir_placeholder()}
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!scanNodeId}
            onclick={() => {
              browserTarget = "scan";
              browserOpen = true;
            }}
            title={m.projects_pick_folder()}
          >
            <FolderOpenIcon class="size-3.5" />
            <span>{m.projects_browse()}</span>
          </Button>
        </div>
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (scanOpen = false)}>
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={scanBusy || scanNodeId === ""}>
          {scanBusy ? m.projects_scanning() : m.projects_scan()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- Manual folder linking -->
<Dialog.Root open={linkOpen} onOpenChange={(open) => (linkOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.projects_link()}</Dialog.Title>
      <Dialog.Description>
        {m.projects_link_dialog_description_prefix()}
        <code>project_key</code>
        {m.projects_link_dialog_description_suffix()}
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitLink}>
      <div class="grid gap-2">
        <Label for="link-node">{m.projects_node()}</Label>
        <select
          id="link-node"
          bind:value={linkNodeId}
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
        >
          {#each sync.nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}{node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
            </option>
          {/each}
        </select>
      </div>
      <div class="grid gap-2">
        <Label for="link-project">{m.projects_project_id()}</Label>
        <Input
          id="link-project"
          bind:value={linkProjectId}
          placeholder="github.com/nazo6/flexagent"
          class="font-mono text-xs"
        />
      </div>
      <div class="grid gap-2">
        <Label for="link-path">{m.projects_target_directory()}</Label>
        <div class="flex items-center gap-1.5">
          <Input
            id="link-path"
            bind:value={linkPath}
            placeholder="D:\\ghq\\github.com\\nazo6\\flexagent"
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 px-2.5 text-xs shrink-0 gap-1.5"
            disabled={!linkNodeId}
            onclick={() => {
              browserTarget = "link";
              browserOpen = true;
            }}
            title={m.projects_pick_folder()}
          >
            <FolderOpenIcon class="size-3.5" />
            <span>{m.projects_browse()}</span>
          </Button>
        </div>
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (linkOpen = false)}>
          {m.common_cancel()}
        </Button>
        <Button
          type="submit"
          disabled={linkBusy ||
            linkNodeId === "" ||
            linkProjectId.trim() === "" ||
            linkPath.trim() === ""}
        >
          {linkBusy ? m.projects_linking() : m.projects_link_action()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<FolderBrowserDialog
  bind:open={browserOpen}
  nodeId={browserNodeId}
  initialPath={browserInitialPath}
  title={browserTarget === "create"
    ? m.projects_pick_worktree_folder()
    : browserTarget === "scan"
      ? m.projects_pick_scan_folder()
      : m.projects_pick_link_folder()}
  onSelect={(path) => {
    if (browserTarget === "create") createPath = path;
    else if (browserTarget === "scan") scanDir = path;
    else linkPath = path;
  }}
/>
