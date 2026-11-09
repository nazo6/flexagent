<script lang="ts">
  import { goto } from "$app/navigation";
  import AddProjectDialog from "#lib/components/projects/AddProjectDialog.svelte";
  import ProjectCard from "#lib/components/projects/ProjectCard.svelte";
  import ProjectProfileDialog from "#lib/components/projects/ProjectProfileDialog.svelte";
  import ProjectScanDialog from "#lib/components/projects/ProjectScanDialog.svelte";
  import WorktreeManagerDialog from "#lib/components/projects/WorktreeManagerDialog.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
  import type { WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { shortId } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { createKeyedResource } from "#lib/stores/resource.svelte.ts";
  import FolderPlusIcon from "@lucide/svelte/icons/folder-plus";
  import LinkIcon from "@lucide/svelte/icons/link";
  import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";

  /** One worktree list per project (loaded on demand). */
  const worktrees = createKeyedResource<string, WorktreeInfo[]>(
    (projectId) => sync.connection.client.worktrees(projectId),
    { onError: (error) => problems.report("load worktrees", error) },
  );

  let worktreeDialog: WorktreeManagerDialog;
  let profileDialog: ProjectProfileDialog;
  let addDialog: AddProjectDialog;
  let scanDialog: ProjectScanDialog;

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }

  /** Open the new session screen for this project (optionally scoped to a node and path). */
  function openNewSession(projectId: string, bindingNodeId?: string, path?: string) {
    const params = new URLSearchParams({ project: projectId });
    if (bindingNodeId) params.set("node", bindingNodeId);
    if (path) params.set("path", path);
    void goto(`/?${params.toString()}`);
  }
</script>

<div class="desk-page">
  <div class="flex flex-wrap items-end justify-between gap-3">
    <div class="flex flex-col gap-1">
      <h1 class="text-xl font-semibold tracking-tight">{m.projects_title()}</h1>
      <p class="text-muted-foreground text-ui">{m.projects_subtitle()}</p>
    </div>
    <div class="flex items-center gap-1.5">
      <Button size="sm" onclick={() => addDialog.show()} title={m.projects_add_folder_title()}>
        <FolderPlusIcon />
        {m.projects_add_folder()}
      </Button>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger
          class="border-border bg-background hover:bg-muted focus-visible:ring-ring/40 inline-flex size-8 items-center justify-center rounded-md border transition-colors outline-none focus-visible:ring-1"
          title={m.projects_more_actions()}
        >
          <MoreHorizontalIcon class="size-4" />
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end" class="w-64">
          <DropdownMenu.Item onSelect={() => addDialog.show(true)}>
            <LinkIcon />
            {m.projects_advanced_link()}
          </DropdownMenu.Item>
          <DropdownMenu.Item onSelect={() => scanDialog.show()}>
            <SearchIcon />
            {m.projects_scan_menu()}
          </DropdownMenu.Item>
          <DropdownMenu.Separator />
          <DropdownMenu.Item onSelect={() => void sync.refreshProjects()}>
            <RefreshCwIcon />
            {m.common_refresh()}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    </div>
  </div>

  {#if sync.projectsLoading && sync.projects.length === 0}
    <p class="text-muted-foreground text-sm">{m.common_loading()}</p>
  {:else if sync.projectsError !== null && sync.projects.length === 0}
    <div
      class="border-danger/30 bg-danger-soft/50 flex flex-col items-start gap-2 rounded-lg border p-3"
    >
      <p class="text-danger text-ui">
        {m.common_load_failed()}
        {sync.projectsError}
      </p>
      <Button variant="outline" size="sm" onclick={() => void sync.refreshProjects()}>
        <RefreshCwIcon />
        {m.common_retry()}
      </Button>
    </div>
  {:else if sync.projects.length === 0}
    <p class="text-muted-foreground text-sm">{m.projects_empty()}</p>
  {:else}
    <div class="border-border flex flex-col border-t">
      {#each sync.projects as project (project.project_id)}
        <ProjectCard
          {project}
          worktreeState={worktrees.entry(project.project_id)}
          {nodeName}
          onRefreshWorktrees={(projectId) => void worktrees.load(projectId)}
          onCreateWorktree={(projectId) => worktreeDialog.openCreate(projectId)}
          onOpenSettings={(entry) => profileDialog.open(entry)}
          onPrune={(projectId) => worktreeDialog.openPrune(projectId)}
          onNewSession={openNewSession}
          onRemoveWorktree={(projectId, worktree) => worktreeDialog.openRemove(projectId, worktree)}
        />
      {/each}
    </div>
  {/if}
</div>

<WorktreeManagerDialog
  bind:this={worktreeDialog}
  nodes={sync.nodes}
  {nodeName}
  onChanged={(projectId) => void worktrees.load(projectId)}
/>
<ProjectProfileDialog bind:this={profileDialog} {nodeName} />
<AddProjectDialog bind:this={addDialog} />
<ProjectScanDialog bind:this={scanDialog} nodes={sync.nodes} />
