<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import type { NodeSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
  import SearchIcon from "@lucide/svelte/icons/search";
  import { toast } from "svelte-sonner";

  let { nodes }: { nodes: NodeSummary[] } = $props();

  let open = $state(false);
  let scanNodeId = $state("");
  let scanDir = $state("");
  let scanBusy = $state(false);
  let browserOpen = $state(false);

  function defaultOnlineNode(): string {
    return nodes.find((node) => node.is_online)?.node_id ?? "";
  }

  export function show(): void {
    scanNodeId = defaultOnlineNode();
    scanDir = "";
    scanBusy = false;
    open = true;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (scanBusy || scanNodeId === "") return;
    scanBusy = true;
    try {
      const previous = new Set(sync.projects.map((project) => project.project_id));
      const response = await sync.connection.client.scanProjects(scanNodeId, {
        dir: scanDir.trim() === "" ? null : scanDir.trim(),
      });
      const added = response.projects.filter((project) => !previous.has(project.project_id)).length;
      toast.success(
        m.projects_scan_complete({
          dirs: response.scanned_dirs.length,
          projects: added,
        }),
      );
      open = false;
      await sync.refreshProjects();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      scanBusy = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <SearchIcon class="size-4" />
        {m.projects_scan_dialog_title()}
      </Dialog.Title>
      <Dialog.Description>{m.projects_scan_dialog_description()}</Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submit}>
      <div class="grid gap-2">
        <Label for="scan-node">{m.projects_node()}</Label>
        <select
          id="scan-node"
          bind:value={scanNodeId}
          class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
        >
          <option value="">{m.common_no_node_online()}</option>
          {#each nodes as node (node.node_id)}
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
            class="flex-1 font-mono text-[13px]"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 shrink-0 gap-1.5 px-2.5 text-[13px]"
            disabled={!scanNodeId}
            onclick={() => (browserOpen = true)}
            title={m.projects_pick_folder()}
          >
            <FolderOpenIcon class="size-3.5" />
            <span>{m.projects_browse()}</span>
          </Button>
        </div>
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (open = false)}>
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={scanBusy || scanNodeId === ""}>
          {scanBusy ? m.projects_scanning() : m.projects_scan()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<FolderBrowserDialog
  bind:open={browserOpen}
  nodeId={scanNodeId}
  initialPath={scanDir}
  title={m.projects_pick_scan_folder()}
  onSelect={(path) => (scanDir = path)}
/>
