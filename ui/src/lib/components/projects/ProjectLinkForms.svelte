<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import type { NodeSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
  import { toast } from "svelte-sonner";

  interface Props {
    nodes: NodeSummary[];
  }

  let { nodes }: Props = $props();

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

  // Shared folder browser
  let browserOpen = $state(false);
  /** Which field receives the folder browser selection. */
  let browserTarget = $state<"scan" | "link">("scan");

  /** Node the folder browser browses on. */
  const browserNodeId = $derived(browserTarget === "scan" ? scanNodeId : linkNodeId);

  /** Initial path for the folder browser. */
  const browserInitialPath = $derived(browserTarget === "scan" ? scanDir : linkPath);

  /** Default online node selection (the first one). */
  function defaultOnlineNode(): string {
    return nodes.find((node) => node.is_online)?.node_id ?? "";
  }

  /** Opens the bulk scan dialog. */
  export function openScan(): void {
    scanNodeId = defaultOnlineNode();
    scanDir = "";
    scanOpen = true;
  }

  /** Opens the manual link dialog. */
  export function openLink(): void {
    linkNodeId = defaultOnlineNode();
    linkProjectId = "";
    linkPath = "";
    linkOpen = true;
  }

  async function submitScan(event: SubmitEvent): Promise<void> {
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

  async function submitLink(event: SubmitEvent): Promise<void> {
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
</script>

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
            class="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-9 shrink-0 gap-1.5 px-2.5 text-xs"
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
          {#each nodes as node (node.node_id)}
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
            class="h-9 shrink-0 gap-1.5 px-2.5 text-xs"
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
  title={browserTarget === "scan" ? m.projects_pick_scan_folder() : m.projects_pick_link_folder()}
  onSelect={(path) => {
    if (browserTarget === "scan") scanDir = path;
    else linkPath = path;
  }}
/>
