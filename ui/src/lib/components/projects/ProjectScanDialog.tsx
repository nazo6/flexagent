import type { JSX } from "@solidjs/web";
import { For, createSignal } from "solid-js";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.tsx";
import type { NodeSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import IconFolderOpen from "~icons/lucide/folder-open";
import IconSearch from "~icons/lucide/search";
import { toast } from "#lib/components/ui/sonner/index.ts";

export interface ProjectScanDialogOptions {
  /** Nodes the scan can run on (the page's `sync.nodes`). */
  nodes: () => NodeSummary[];
}

export interface ProjectScanDialogHandle {
  /** Opens the dialog with the first online node preselected. */
  show: () => void;
  /** The dialog itself; render it once inside the page. */
  View: () => JSX.Element;
}

/**
 * Scan-for-projects dialog, ported from `ProjectScanDialog.svelte`.
 *
 * Like the Svelte version it only asks for a node and an optional directory:
 * the node walks its configured scan roots when the directory is empty.
 */
export function createProjectScanDialog(
  options: ProjectScanDialogOptions,
): ProjectScanDialogHandle {
  const [open, setOpen] = createSignal(false);
  const [scanNodeId, setScanNodeId] = createSignal("");
  const [scanDir, setScanDir] = createSignal("");
  const [scanBusy, setScanBusy] = createSignal(false);
  const [browserOpen, setBrowserOpen] = createSignal(false);

  function defaultOnlineNode(): string {
    return options.nodes().find((node) => node.is_online)?.node_id ?? "";
  }

  function show(): void {
    setScanNodeId(defaultOnlineNode());
    setScanDir("");
    setScanBusy(false);
    setOpen(true);
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const nodeId = scanNodeId();
    if (scanBusy() || nodeId === "") return;
    setScanBusy(true);
    try {
      const previous = new Set(sync.projects.map((project) => project.project_id));
      const response = await sync.connection.client.scanProjects(nodeId, {
        dir: scanDir().trim() === "" ? null : scanDir().trim(),
      });
      const added = response.projects.filter((project) => !previous.has(project.project_id)).length;
      toast.success(
        m.projects_scan_complete({
          dirs: response.scanned_dirs.length,
          projects: added,
        }),
      );
      setOpen(false);
      await sync.refreshProjects();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setScanBusy(false);
    }
  }

  function View() {
    return (
      <>
        <Dialog.Root open={open()} onOpenChange={setOpen}>
          <Dialog.Content class="sm:max-w-md">
            <Dialog.Header>
              <Dialog.Title class="flex items-center gap-2">
                <IconSearch class="size-4" />
                {m.projects_scan_dialog_title()}
              </Dialog.Title>
              <Dialog.Description>{m.projects_scan_dialog_description()}</Dialog.Description>
            </Dialog.Header>
            <form class="flex flex-col gap-4" onSubmit={submit}>
              <div class="grid gap-2">
                <Label for="scan-node">{m.projects_node()}</Label>
                <select
                  id="scan-node"
                  value={scanNodeId()}
                  onChange={(event) => setScanNodeId(event.currentTarget.value)}
                  class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
                >
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
                </select>
              </div>
              <div class="grid gap-2">
                <Label for="scan-dir">{m.projects_scan_dir()}</Label>
                <div class="flex items-center gap-1.5">
                  <Input
                    id="scan-dir"
                    value={scanDir()}
                    onValueChange={setScanDir}
                    placeholder={m.projects_scan_dir_placeholder()}
                    class="flex-1 font-mono text-[13px]"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    class="h-9 shrink-0 gap-1.5 px-2.5 text-[13px]"
                    disabled={scanNodeId() === ""}
                    onClick={() => setBrowserOpen(true)}
                    title={m.projects_pick_folder()}
                  >
                    <IconFolderOpen class="size-3.5" />
                    <span>{m.projects_browse()}</span>
                  </Button>
                </div>
              </div>
              <Dialog.Footer>
                <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                  {m.common_cancel()}
                </Button>
                <Button type="submit" disabled={scanBusy() || scanNodeId() === ""}>
                  {scanBusy() ? m.projects_scanning() : m.projects_scan()}
                </Button>
              </Dialog.Footer>
            </form>
          </Dialog.Content>
        </Dialog.Root>

        <FolderBrowserDialog
          open={browserOpen()}
          onOpenChange={setBrowserOpen}
          nodeId={scanNodeId()}
          initialPath={scanDir()}
          title={m.projects_pick_scan_folder()}
          onSelect={(path) => setScanDir(path)}
        />
      </>
    );
  }

  return { show, View };
}
