import { For, Match, Switch, createEffect } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { createAddProjectDialog } from "#lib/components/projects/AddProjectDialog.tsx";
import { ProjectCard } from "#lib/components/projects/ProjectCard.tsx";
import { createProjectScanDialog } from "#lib/components/projects/ProjectScanDialog.tsx";
import { createWorktreeManagerDialog } from "#lib/components/projects/WorktreeManagerDialog.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type { WorktreeInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { shortId } from "#lib/session-status.ts";
import { sync } from "#lib/stores/app.ts";
import { problems } from "#lib/stores/problems.ts";
import { createKeyedResource } from "#lib/stores/resource.ts";
import IconFolderPlus from "~icons/lucide/folder-plus";
import IconLink from "~icons/lucide/link";
import IconMoreHorizontal from "~icons/lucide/more-horizontal";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconSearch from "~icons/lucide/search";

export default function ProjectsPage() {
  const navigate = useNavigate();

  /** One worktree list per project (loaded on demand). */
  const worktrees = createKeyedResource<string, WorktreeInfo[]>(
    (projectId) => sync.connection.client.worktrees(projectId),
    { onError: (error) => problems.report("load worktrees", error) },
  );

  const worktreeDialog = createWorktreeManagerDialog({
    nodes: () => sync.nodes,
    nodeName,
    onChanged: (projectId) => void worktrees.load(projectId),
  });
  const addDialog = createAddProjectDialog();
  const scanDialog = createProjectScanDialog({ nodes: () => sync.nodes });

  createEffect(
    () => m.projects_page_title(),
    (title) => {
      document.title = title;
    },
  );

  /** Removes this node's folder binding; the project disappears with its last binding. */
  async function unlinkBinding(_projectId: string, nodeId: string, path: string): Promise<void> {
    try {
      await sync.connection.client.unlinkProject(nodeId, path);
      toast.success(m.projects_unlinked({ path }));
      await sync.refreshProjects();
    } catch (error) {
      problems.report("unlink project", error);
    }
  }

  function nodeName(nodeId: string): string {
    return sync.nodes.find((node) => node.node_id === nodeId)?.name ?? shortId(nodeId);
  }

  /** Open the new session screen for this project (optionally scoped to a node and path). */
  function openNewSession(projectId: string, bindingNodeId?: string, path?: string): void {
    const params = new URLSearchParams({ project: projectId });
    if (bindingNodeId) params.set("node", bindingNodeId);
    if (path) params.set("path", path);
    void navigate(`/?${params.toString()}`);
  }

  return (
    <>
      <div class="desk-page">
        <div class="flex flex-wrap items-end justify-between gap-3">
          <div class="flex flex-col gap-1">
            <h1 class="text-xl font-semibold tracking-tight">{m.projects_title()}</h1>
            <p class="text-muted-foreground text-[13px]">{m.projects_subtitle()}</p>
          </div>
          <div class="flex items-center gap-1.5">
            <Button
              size="sm"
              onClick={() => addDialog.show()}
              title={m.projects_add_folder_title()}
            >
              <IconFolderPlus />
              {m.projects_add_folder()}
            </Button>
            <DropdownMenu.Root>
              <DropdownMenu.Trigger
                class="border-border bg-background hover:bg-muted focus-visible:ring-ring/40 inline-flex size-8 items-center justify-center rounded-md border transition-colors outline-none focus-visible:ring-1"
                title={m.projects_more_actions()}
              >
                <IconMoreHorizontal class="size-4" />
              </DropdownMenu.Trigger>
              <DropdownMenu.Content align="end" class="w-64">
                <DropdownMenu.Item onSelect={() => addDialog.show(true)}>
                  <IconLink />
                  {m.projects_advanced_link()}
                </DropdownMenu.Item>
                <DropdownMenu.Item onSelect={() => scanDialog.show()}>
                  <IconSearch />
                  {m.projects_scan_menu()}
                </DropdownMenu.Item>
                <DropdownMenu.Separator />
                <DropdownMenu.Item onSelect={() => void sync.refreshProjects()}>
                  <IconRefreshCw />
                  {m.common_refresh()}
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Root>
          </div>
        </div>

        <Switch>
          <Match when={sync.projectsLoading && sync.projects.length === 0}>
            <p class="text-muted-foreground text-sm">{m.common_loading()}</p>
          </Match>
          <Match when={sync.projectsError !== null && sync.projects.length === 0}>
            <div class="border-danger/30 bg-danger-soft/50 flex flex-col items-start gap-2 rounded-lg border p-3">
              <p class="text-danger text-[13px]">
                {m.common_load_failed()} {sync.projectsError}
              </p>
              <Button variant="outline" size="sm" onClick={() => void sync.refreshProjects()}>
                <IconRefreshCw />
                {m.common_retry()}
              </Button>
            </div>
          </Match>
          <Match when={sync.projects.length === 0}>
            <p class="text-muted-foreground text-sm">{m.projects_empty()}</p>
          </Match>
          <Match when={true}>
            <div class="border-border flex flex-col border-t">
              <For each={sync.projects}>
                {(project) => (
                  <ProjectCard
                    project={project}
                    worktreeState={worktrees.entry(project.project_id)}
                    nodeName={nodeName}
                    onRefreshWorktrees={(projectId) => void worktrees.load(projectId)}
                    onCreateWorktree={(projectId) => worktreeDialog.openCreate(projectId)}
                    onPrune={(projectId) => worktreeDialog.openPrune(projectId)}
                    onNewSession={openNewSession}
                    onRemoveWorktree={(projectId, worktree) =>
                      worktreeDialog.openRemove(projectId, worktree)
                    }
                    onUnlinkBinding={(projectId, nodeId, path) =>
                      void unlinkBinding(projectId, nodeId, path)
                    }
                  />
                )}
              </For>
            </div>
          </Match>
        </Switch>
      </div>

      <worktreeDialog.View />
      <addDialog.View />
      <scanDialog.View />
    </>
  );
}
