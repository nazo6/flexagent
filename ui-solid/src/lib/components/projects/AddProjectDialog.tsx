import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, createEffect, createMemo, createSignal } from "solid-js";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import FolderPicker from "#lib/components/fs/FolderPicker.tsx";
import { getNodeAvailability } from "#lib/node-status.ts";
import type { ProjectPreview } from "#lib/generated/bindings.ts";
import type { QuickPath } from "#lib/folder-picker.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconAlertTriangle from "~icons/lucide/alert-triangle";
import IconFolderPlus from "~icons/lucide/folder-plus";
import IconGitBranch from "~icons/lucide/git-branch";
import IconLoaderCircle from "~icons/lucide/loader-circle";

/** Result passed to the caller after a folder was added or selected. */
export interface AddedProject {
  /** Logical project ID */
  projectId: string;
  /** Directory added on the node */
  localPath: string;
  /** Node that owns the folder */
  nodeId: string;
}

export interface AddProjectDialogOptions {
  /** Preferred node when the dialog opens (typically the new-session node). */
  defaultNodeId?: string;
  /** Called after a folder was added, or when an already-added project is selected. */
  onAdded?: (project: AddedProject) => void;
}

export interface AddProjectDialogHandle {
  /** Opens the dialog. Pass `true` to start in the advanced custom-key mode. */
  show: (withAdvanced?: boolean) => void;
  /** The dialog itself; render it once inside the page. */
  View: () => JSX.Element;
}

function pathBasename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/**
 * Add-folder dialog, ported from `AddProjectDialog.svelte`.
 *
 * The Svelte page reached the dialog through `bind:this` and called `show()`;
 * here the factory owns the state and returns that method plus the element to
 * render, which keeps the imperative entry point without a component ref.
 */
export function createAddProjectDialog(
  options: AddProjectDialogOptions = {},
): AddProjectDialogHandle {
  const [open, setOpen] = createSignal(false);
  const [selectedNodeId, setSelectedNodeId] = createSignal("");
  const [targetPath, setTargetPath] = createSignal("");
  const [preview, setPreview] = createSignal<ProjectPreview | null>(null);
  const [previewLoading, setPreviewLoading] = createSignal(false);
  const [previewError, setPreviewError] = createSignal<string | null>(null);
  /** Guards in-flight previews against a newer selection. */
  let previewGeneration = 0;
  const [advanced, setAdvanced] = createSignal(false);
  const [customKey, setCustomKey] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [formError, setFormError] = createSignal<string | null>(null);

  const nodes = () => sync.nodes;
  const onlineNodes = createMemo(() => nodes().filter((node) => node.is_online));
  const selectedNode = createMemo(
    () => nodes().find((node) => node.node_id === selectedNodeId()) ?? null,
  );
  const selectedAvailability = createMemo(() =>
    getNodeAvailability(selectedNode(), sync.wsConnected),
  );

  function pickDefaultNode(): string {
    const preferred = nodes().find(
      (node) => node.node_id === options.defaultNodeId && node.is_online,
    );
    return preferred?.node_id ?? onlineNodes()[0]?.node_id ?? "";
  }

  const quickPaths = createMemo<QuickPath[]>(() => {
    const entries: (QuickPath & { at: number })[] = [];
    for (const project of sync.projects) {
      for (const binding of project.bindings) {
        if (binding.node_id !== selectedNodeId()) continue;
        entries.push({
          label: project.name,
          path: binding.local_path,
          description: m.projects_quick_project(),
          at: binding.last_used_at,
        });
      }
    }
    for (const session of sync.sessions) {
      if (session.node_id !== selectedNodeId()) continue;
      entries.push({
        label: pathBasename(session.local_path),
        path: session.local_path,
        description: m.projects_quick_session(),
        at: session.updated_at,
      });
    }
    const byPath = new Map<string, QuickPath & { at: number }>();
    for (const entry of entries) {
      const previous = byPath.get(entry.path);
      if (previous === undefined || previous.at < entry.at) byPath.set(entry.path, entry);
    }
    return [...byPath.values()]
      .toSorted((left, right) => right.at - left.at)
      .slice(0, 6)
      .map(({ label, path, description }) => ({ label, path, description }));
  });

  const pathBinding = createMemo(() => {
    const current = preview();
    if (current === null) return null;
    for (const project of sync.projects) {
      for (const binding of project.bindings) {
        if (binding.node_id === selectedNodeId() && binding.local_path === current.local_path) {
          return { project, binding };
        }
      }
    }
    return null;
  });
  const alreadyAdded = createMemo(() => {
    const binding = pathBinding();
    return binding !== null && binding.project.project_id === preview()?.project_id;
  });
  const conflictingBinding = createMemo(() => {
    const binding = pathBinding();
    return binding !== null && binding.project.project_id !== preview()?.project_id
      ? binding
      : null;
  });
  const existingProject = createMemo(() => {
    const current = preview();
    if (current === null) return null;
    return sync.projects.find((project) => project.project_id === current.project_id) ?? null;
  });

  const effectiveProjectId = createMemo(() => {
    const key = customKey().trim();
    if (advanced() && key !== "") {
      return key.includes("/") ? key : `user:${key}`;
    }
    return preview()?.project_id ?? "";
  });

  const canSubmit = createMemo(
    () =>
      !busy() &&
      !previewLoading() &&
      previewError() === null &&
      preview() !== null &&
      selectedNodeId() !== "" &&
      selectedAvailability().isAvailable &&
      (!advanced() || customKey().trim() !== ""),
  );

  // Debounced identity preview for the folder currently under the picker.
  createEffect(
    () => [open(), selectedNodeId(), targetPath()] as const,
    ([isOpen, node, path]) => {
      previewGeneration += 1;
      const generation = previewGeneration;
      if (!isOpen || node === "" || path.trim() === "") {
        setPreview(null);
        setPreviewLoading(false);
        setPreviewError(null);
        return;
      }
      setPreviewLoading(true);
      setPreviewError(null);
      const timer = setTimeout(() => {
        void (async () => {
          try {
            const result = await sync.connection.client.inspectProject(node, path);
            if (generation !== previewGeneration) return;
            setPreview(result);
          } catch (error) {
            if (generation !== previewGeneration) return;
            setPreview(null);
            setPreviewError(errorMessage(error));
          } finally {
            if (generation === previewGeneration) setPreviewLoading(false);
          }
        })();
      }, 250);
      return () => clearTimeout(timer);
    },
  );

  function show(withAdvanced = false): void {
    setAdvanced(withAdvanced);
    setCustomKey("");
    setTargetPath("");
    setPreview(null);
    setPreviewError(null);
    setPreviewLoading(false);
    previewGeneration += 1;
    setFormError(null);
    setBusy(false);
    setSelectedNodeId(pickDefaultNode());
    setOpen(true);
  }

  function onNodeChange(): void {
    setTargetPath("");
    setPreview(null);
    setPreviewError(null);
    setFormError(null);
  }

  async function submit(): Promise<void> {
    const current = preview();
    if (!canSubmit() || current === null) return;
    if (alreadyAdded()) {
      options.onAdded?.({
        projectId: current.project_id,
        localPath: current.local_path,
        nodeId: selectedNodeId(),
      });
      setOpen(false);
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      const result = advanced()
        ? await sync.connection.client.linkProject(selectedNodeId(), {
            project_id: customKey().trim(),
            local_path: targetPath(),
          })
        : await sync.connection.client.addProject(selectedNodeId(), targetPath());
      await sync.refreshProjects();
      toast.success(
        m.projects_add_success({
          project: current.name,
          path: result.local_path,
        }),
      );
      setOpen(false);
      options.onAdded?.({
        projectId: result.project_id,
        localPath: result.local_path,
        nodeId: selectedNodeId(),
      });
    } catch (error) {
      const message = errorMessage(error);
      setFormError(message);
      toast.error(message);
    } finally {
      setBusy(false);
    }
  }

  function identityLabel(): string {
    if (advanced() && customKey().trim() !== "") return m.projects_identity_user_key();
    switch (preview()?.identity.kind) {
      case "git_remote":
        return m.projects_identity_git_remote();
      case "user_key":
        return m.projects_identity_user_key();
      case "local":
        return m.projects_identity_local();
      default:
        return m.projects_identity_unknown();
    }
  }

  function View() {
    return (
      <Dialog.Root open={open()} onOpenChange={setOpen}>
        <Dialog.Content class="flex max-h-[88vh] flex-col gap-4 p-5 sm:max-w-4xl">
          <Dialog.Header class="gap-1">
            <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
              <IconFolderPlus class="text-primary size-4" />
              {m.projects_add_folder_title()}
            </Dialog.Title>
            <Dialog.Description class="text-muted-foreground text-[13px]">
              {m.projects_add_folder_description()}
            </Dialog.Description>
          </Dialog.Header>

          <div class="flex flex-col gap-1.5">
            <Label for="add-project-node">{m.projects_node()}</Label>
            <Show
              when={nodes().length > 0}
              fallback={<p class="text-destructive text-[13px]">{m.projects_no_nodes()}</p>}
            >
              <select
                id="add-project-node"
                value={selectedNodeId()}
                onChange={(event) => {
                  setSelectedNodeId(event.currentTarget.value);
                  onNodeChange();
                }}
                class="border-input bg-surface-raised focus-visible:ring-ring/40 h-8 w-full max-w-xs rounded-md border px-2 text-[13px] outline-none focus-visible:ring-1"
              >
                <option value="">
                  {onlineNodes().length === 0
                    ? m.common_no_node_online()
                    : m.settings_select_node()}
                </option>
                <For each={nodes()}>
                  {(node) => (
                    <option value={node.node_id} disabled={!node.is_online}>
                      {node.name}
                      {node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
                    </option>
                  )}
                </For>
              </select>
            </Show>
          </div>

          <div class="grid min-h-0 flex-1 gap-4 md:grid-cols-[minmax(0,1fr)_260px]">
            <div class="flex min-h-64 flex-col md:min-h-0">
              <Show
                when={selectedNodeId() !== ""}
                fallback={
                  <div class="text-muted-foreground flex min-h-48 items-center justify-center rounded-md border border-dashed p-4 text-center text-[13px]">
                    {m.projects_no_nodes()}
                  </div>
                }
              >
                <FolderPicker
                  nodeId={selectedNodeId()}
                  active={open()}
                  mode="navigate"
                  quickPaths={quickPaths()}
                  class="h-full"
                  onpathchange={(path) => setTargetPath(path)}
                  onerror={(message) => {
                    setTargetPath("");
                    setPreview(null);
                    setPreviewError(message);
                  }}
                />
              </Show>
            </div>

            <div class="bg-background flex min-h-48 flex-col gap-2 rounded-md border p-3 text-[13px]">
              <Switch
                fallback={
                  <div class="text-muted-foreground flex flex-1 items-center justify-center text-center">
                    {m.projects_choose_folder_hint()}
                  </div>
                }
              >
                <Match when={previewLoading()}>
                  <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2">
                    <IconLoaderCircle class="size-4 animate-spin" />
                    {m.projects_inspecting()}
                  </div>
                </Match>
                <Match when={previewError()}>
                  {(message) => (
                    <div class="text-destructive flex flex-1 flex-col items-center justify-center gap-2 text-center">
                      <IconAlertTriangle class="size-5" />
                      <span>{m.projects_inspect_failed()}</span>
                      <span class="text-[11px] break-words opacity-80">{message()}</span>
                    </div>
                  )}
                </Match>
                <Match when={preview()}>
                  {(current) => (
                    <>
                      <span class="text-muted-foreground w-fit rounded-full border px-2 py-0.5 text-[11px]">
                        {identityLabel()}
                      </span>
                      <h3 class="text-sm font-semibold">{current().name}</h3>
                      <code
                        class="text-muted-foreground block truncate text-[11px]"
                        title={effectiveProjectId()}
                      >
                        {effectiveProjectId()}
                      </code>
                      <Show when={current().canonical_git_url}>
                        <p
                          class="text-muted-foreground truncate font-mono text-[11px]"
                          title={current().canonical_git_url ?? ""}
                        >
                          {current().canonical_git_url}
                        </p>
                      </Show>
                      <Show when={current().relative_subpath}>
                        <p class="text-muted-foreground text-[11px]">
                          {m.projects_monorepo_subpath({ path: current().relative_subpath ?? "" })}
                        </p>
                      </Show>
                      <Show when={current().is_worktree}>
                        <p class="text-muted-foreground flex items-center gap-1 text-[11px]">
                          <IconGitBranch class="size-3" />
                          {m.projects_worktree_binding()}
                        </p>
                      </Show>
                      <p
                        class="text-muted-foreground truncate font-mono text-[11px]"
                        title={current().local_path}
                      >
                        {current().local_path}
                      </p>
                      <div class="mt-auto flex flex-col gap-1 border-t pt-2 text-[11px] leading-relaxed">
                        <Switch>
                          <Match when={alreadyAdded()}>
                            <p class="text-muted-foreground">{m.projects_already_added()}</p>
                          </Match>
                          <Match when={conflictingBinding()}>
                            {(binding) => (
                              <p class="text-warn">
                                {m.projects_binding_conflict({ project: binding().project.name })}
                              </p>
                            )}
                          </Match>
                          <Match when={existingProject()}>
                            <p class="text-muted-foreground">
                              {m.projects_existing_project_add_binding()}
                            </p>
                          </Match>
                          <Match when={true}>
                            <p class="text-muted-foreground">{m.projects_new_project_hint()}</p>
                          </Match>
                        </Switch>
                      </div>
                    </>
                  )}
                </Match>
              </Switch>
            </div>
          </div>

          <div class="border-t pt-3">
            <label class="text-muted-foreground flex items-center gap-2 text-[13px]">
              <input
                type="checkbox"
                class="border-input accent-primary size-3.5 rounded border"
                checked={advanced()}
                onChange={(event) => setAdvanced(event.currentTarget.checked)}
              />
              {m.projects_advanced_custom_key()}
            </label>
            <Show when={advanced()}>
              <div class="mt-2 flex max-w-md flex-col gap-1">
                <Input
                  value={customKey()}
                  onValueChange={setCustomKey}
                  placeholder="my-project"
                  class="h-8 font-mono text-[13px]"
                  disabled={busy()}
                />
                <p class="text-muted-foreground text-[11px]">{m.projects_custom_key_hint()}</p>
              </div>
            </Show>
          </div>

          <Show when={formError()}>
            {(message) => <p class="text-destructive text-[13px]">{message()}</p>}
          </Show>

          <Dialog.Footer class="gap-2 sm:gap-2">
            <Button variant="outline" size="sm" onClick={() => setOpen(false)} disabled={busy()}>
              {m.common_cancel()}
            </Button>
            <Button
              size="sm"
              disabled={!canSubmit() && !alreadyAdded()}
              onClick={() => void submit()}
            >
              <Switch>
                <Match when={busy()}>
                  <IconLoaderCircle class="size-3.5 animate-spin" />
                  {m.projects_adding()}
                </Match>
                <Match when={alreadyAdded()}>{m.projects_use_this_project()}</Match>
                <Match when={advanced()}>{m.projects_link_action()}</Match>
                <Match when={true}>{m.projects_add_action()}</Match>
              </Switch>
            </Button>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Root>
    );
  }

  return { show, View };
}
