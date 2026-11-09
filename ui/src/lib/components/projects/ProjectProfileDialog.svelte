<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { ProjectSummary } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";

  interface Props {
    nodeName: (nodeId: string) => string;
  }

  let { nodeName }: Props = $props();

  // Project settings (identity / explicit project key)
  let projectSettings = $state<ProjectSummary | null>(null);
  let projectKey = $state("");
  let projectSettingsBusy = $state(false);

  /** Opens the settings dialog for a project. */
  export function open(project: ProjectSummary): void {
    projectSettings = project;
    projectKey = project.identity.key ?? "";
  }

  async function saveProjectKey(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const project = projectSettings;
    if (project === null || projectSettingsBusy) return;
    const key = projectKey.trim();
    if (key === "") return;
    projectSettingsBusy = true;
    try {
      for (const binding of project.bindings) {
        await sync.connection.client.linkProject(binding.node_id, {
          project_id: key,
          local_path: binding.local_path,
        });
      }
      await sync.refreshProjects();
      projectSettings = null;
      toast.success(`Linked this project to key ${key}.`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      projectSettingsBusy = false;
    }
  }
</script>

<Dialog.Root
  open={projectSettings !== null}
  onOpenChange={(open) => !open && (projectSettings = null)}
>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>Project settings</Dialog.Title>
      <Dialog.Description>
        Link this project to an explicit key shared by every machine. The display name is derived
        automatically from the identity.
      </Dialog.Description>
    </Dialog.Header>
    {#if projectSettings}
      <div class="flex flex-col gap-3 text-sm">
        <div class="grid gap-1">
          <span class="text-muted-foreground text-ui">Display name</span>
          <span class="font-medium">{projectSettings.name}</span>
        </div>
        <div class="grid gap-1">
          <span class="text-muted-foreground text-ui">Identity</span>
          <span>{projectSettings.identity.kind}</span>
        </div>
        <form class="grid gap-2" onsubmit={saveProjectKey}>
          <Label for="project-key">Project key</Label>
          <div class="flex gap-1.5">
            <Input
              id="project-key"
              bind:value={projectKey}
              placeholder="my-project"
              class="font-mono text-ui"
            />
            <Button type="submit" disabled={projectSettingsBusy || projectKey.trim() === ""}>
              Save
            </Button>
          </div>
          <p class="text-muted-foreground text-ui">
            Use 1-64 characters of [a-zA-Z0-9._-]. To share a project across machines, set the same
            key on each machine.
          </p>
        </form>
        <div class="grid gap-1">
          <span class="text-muted-foreground text-ui">Bindings</span>
          <ul class="flex list-none flex-col gap-1 p-0 text-ui">
            {#each projectSettings.bindings as binding (`${binding.node_id}|${binding.local_path}`)}
              <li class="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
                <Badge variant="outline">{nodeName(binding.node_id)}</Badge>
                <code class="truncate">{binding.local_path}</code>
              </li>
            {/each}
          </ul>
        </div>
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (projectSettings = null)}>Close</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
