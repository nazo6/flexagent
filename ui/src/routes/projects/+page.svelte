<script lang="ts">
  import { goto } from "$app/navigation";
  import ProjectCard from "#lib/components/projects/ProjectCard.svelte";
  import ProjectLinkForms from "#lib/components/projects/ProjectLinkForms.svelte";
  import ProjectProfileDialog from "#lib/components/projects/ProjectProfileDialog.svelte";
  import WorktreeManagerDialog from "#lib/components/projects/WorktreeManagerDialog.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import type { WorktreeInfo } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { shortId } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { createKeyedResource } from "#lib/stores/resource.svelte.ts";
  import LinkIcon from "@lucide/svelte/icons/link";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";

  /** One worktree list per project (loaded on demand). */
  const worktrees = createKeyedResource<string, WorktreeInfo[]>(
    (projectId) => sync.connection.client.worktrees(projectId),
    { onError: (error) => problems.report("load worktrees", error) },
  );

  let worktreeDialog: WorktreeManagerDialog;
  let profileDialog: ProjectProfileDialog;
  let linkForms: ProjectLinkForms;

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

<div class="mx-auto flex max-w-5xl flex-col gap-4 p-4 md:p-6">
  <div class="flex items-center justify-between gap-2">
    <div>
      <h1 class="text-xl font-semibold">{m.projects_title()}</h1>
      <p class="text-muted-foreground text-sm">{m.projects_subtitle()}</p>
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button
        variant="outline"
        size="sm"
        onclick={() => linkForms.openScan()}
        title={m.projects_scan_title()}
      >
        <SearchIcon />
        {m.projects_scan()}
      </Button>
      <Button
        variant="outline"
        size="sm"
        onclick={() => linkForms.openLink()}
        title={m.projects_link_title()}
      >
        <LinkIcon />
        {m.projects_link()}
      </Button>
      <Button variant="outline" size="sm" onclick={() => void sync.refreshProjects()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
    </div>
  </div>

  {#if sync.projectsLoading && sync.projects.length === 0}
    <p class="text-muted-foreground text-sm">{m.common_loading()}</p>
  {:else if sync.projectsError !== null && sync.projects.length === 0}
    <div class="flex flex-col items-start gap-2 rounded-md border border-destructive/40 p-3">
      <p class="text-destructive text-sm">
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
  {/if}
</div>

<WorktreeManagerDialog
  bind:this={worktreeDialog}
  nodes={sync.nodes}
  {nodeName}
  onChanged={(projectId) => void worktrees.load(projectId)}
/>
<ProjectProfileDialog bind:this={profileDialog} {nodeName} />
<ProjectLinkForms bind:this={linkForms} nodes={sync.nodes} />
