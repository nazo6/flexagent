<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import FolderPicker from "#lib/components/fs/FolderPicker.svelte";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import type { ProjectPreview } from "#lib/generated/bindings.ts";
  import type { QuickPath } from "#lib/folder-picker.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import FolderPlusIcon from "@lucide/svelte/icons/folder-plus";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";

  /** Result passed to the caller after a folder was added or selected. */
  interface AddedProject {
    /** Logical project ID */
    projectId: string;
    /** Directory added on the node */
    localPath: string;
    /** Node that owns the folder */
    nodeId: string;
  }

  let {
    defaultNodeId = "",
    onAdded,
  }: {
    /** Preferred node when the dialog opens (typically the new-session node). */
    defaultNodeId?: string;
    /** Called after a folder was added, or when an already-added project is selected. */
    onAdded?: (project: AddedProject) => void;
  } = $props();

  let open = $state(false);
  let selectedNodeId = $state("");
  let targetPath = $state("");
  let preview = $state<ProjectPreview | null>(null);
  let previewLoading = $state(false);
  let previewError = $state<string | null>(null);
  let previewGeneration = 0;
  let advanced = $state(false);
  let customKey = $state("");
  let busy = $state(false);
  let formError = $state<string | null>(null);

  const nodes = $derived(sync.nodes);
  const onlineNodes = $derived(nodes.filter((node) => node.is_online));
  const selectedNode = $derived(nodes.find((node) => node.node_id === selectedNodeId) ?? null);
  const selectedAvailability = $derived(getNodeAvailability(selectedNode));

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }

  function pickDefaultNode(): string {
    const preferred = nodes.find((node) => node.node_id === defaultNodeId && node.is_online);
    return preferred?.node_id ?? onlineNodes[0]?.node_id ?? "";
  }

  const quickPaths = $derived.by<QuickPath[]>(() => {
    const entries: (QuickPath & { at: number })[] = [];
    for (const project of sync.projects) {
      for (const binding of project.bindings) {
        if (binding.node_id !== selectedNodeId) continue;
        entries.push({
          label: project.name,
          path: binding.local_path,
          description: m.projects_quick_project(),
          at: binding.last_used_at,
        });
      }
    }
    for (const session of sync.sessions) {
      if (session.node_id !== selectedNodeId) continue;
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

  const pathBinding = $derived.by(() => {
    if (preview === null) return null;
    for (const project of sync.projects) {
      for (const binding of project.bindings) {
        if (binding.node_id === selectedNodeId && binding.local_path === preview.local_path) {
          return { project, binding };
        }
      }
    }
    return null;
  });
  const alreadyAdded = $derived(
    pathBinding !== null && pathBinding.project.project_id === preview?.project_id,
  );
  const conflictingBinding = $derived(
    pathBinding !== null && pathBinding.project.project_id !== preview?.project_id
      ? pathBinding
      : null,
  );
  const existingProject = $derived.by(() => {
    const current = preview;
    if (current === null) return null;
    return sync.projects.find((project) => project.project_id === current.project_id) ?? null;
  });

  const effectiveProjectId = $derived(
    advanced && customKey.trim() !== ""
      ? customKey.includes("/")
        ? customKey.trim()
        : `user:${customKey.trim()}`
      : (preview?.project_id ?? ""),
  );

  const canSubmit = $derived(
    !busy &&
      !previewLoading &&
      previewError === null &&
      preview !== null &&
      selectedNodeId !== "" &&
      selectedAvailability.isAvailable &&
      (!advanced || customKey.trim() !== ""),
  );

  $effect(() => {
    const node = selectedNodeId;
    const path = targetPath;
    previewGeneration += 1;
    const generation = previewGeneration;
    if (!open || node === "" || path.trim() === "") {
      preview = null;
      previewLoading = false;
      previewError = null;
      return;
    }
    previewLoading = true;
    previewError = null;
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const result = await sync.connection.client.inspectProject(node, path);
          if (generation !== previewGeneration) return;
          preview = result;
        } catch (error) {
          if (generation !== previewGeneration) return;
          preview = null;
          previewError = errorMessage(error);
        } finally {
          if (generation === previewGeneration) previewLoading = false;
        }
      })();
    }, 250);
    return () => clearTimeout(timer);
  });

  /** Opens the dialog. Pass `true` to start in the advanced custom-key mode. */
  export function show(withAdvanced = false): void {
    advanced = withAdvanced;
    customKey = "";
    targetPath = "";
    preview = null;
    previewError = null;
    previewLoading = false;
    previewGeneration += 1;
    formError = null;
    busy = false;
    selectedNodeId = pickDefaultNode();
    open = true;
  }

  function onNodeChange(): void {
    targetPath = "";
    preview = null;
    previewError = null;
    formError = null;
  }

  async function submit(): Promise<void> {
    if (!canSubmit || preview === null) return;
    if (alreadyAdded) {
      onAdded?.({
        projectId: preview.project_id,
        localPath: preview.local_path,
        nodeId: selectedNodeId,
      });
      open = false;
      return;
    }
    busy = true;
    formError = null;
    try {
      const result = advanced
        ? await sync.connection.client.linkProject(selectedNodeId, {
            project_id: customKey.trim(),
            local_path: targetPath,
          })
        : await sync.connection.client.addProject(selectedNodeId, targetPath);
      await sync.refreshProjects();
      toast.success(
        m.projects_add_success({
          project: preview.name,
          path: result.local_path,
        }),
      );
      open = false;
      onAdded?.({
        projectId: result.project_id,
        localPath: result.local_path,
        nodeId: selectedNodeId,
      });
    } catch (error) {
      formError = errorMessage(error);
      toast.error(formError);
    } finally {
      busy = false;
    }
  }

  function identityLabel(): string {
    if (advanced && customKey.trim() !== "") return m.projects_identity_user_key();
    switch (preview?.identity.kind) {
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
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[88vh] flex-col gap-4 p-5 sm:max-w-4xl">
    <Dialog.Header class="gap-1">
      <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
        <FolderPlusIcon class="text-primary size-4" />
        {m.projects_add_folder_title()}
      </Dialog.Title>
      <Dialog.Description class="text-muted-foreground text-[13px]">
        {m.projects_add_folder_description()}
      </Dialog.Description>
    </Dialog.Header>

    <div class="flex flex-col gap-1.5">
      <Label for="add-project-node">{m.projects_node()}</Label>
      {#if nodes.length === 0}
        <p class="text-destructive text-[13px]">{m.projects_no_nodes()}</p>
      {:else}
        <select
          id="add-project-node"
          bind:value={selectedNodeId}
          onchange={onNodeChange}
          class="border-input bg-surface-raised focus-visible:ring-ring/40 h-8 w-full max-w-xs rounded-md border px-2 text-[13px] outline-none focus-visible:ring-1"
        >
          <option value=""
            >{onlineNodes.length === 0
              ? m.common_no_node_online()
              : m.settings_select_node()}</option
          >
          {#each nodes as node (node.node_id)}
            <option value={node.node_id} disabled={!node.is_online}>
              {node.name}{node.is_online ? "" : ` ${m.projects_offline_suffix()}`}
            </option>
          {/each}
        </select>
      {/if}
    </div>

    <div class="grid min-h-0 flex-1 gap-4 md:grid-cols-[minmax(0,1fr)_260px]">
      <div class="flex min-h-64 flex-col md:min-h-0">
        {#if selectedNodeId !== ""}
          <FolderPicker
            nodeId={selectedNodeId}
            active={open}
            mode="navigate"
            {quickPaths}
            class="h-full"
            onpathchange={(path) => (targetPath = path)}
            onerror={(message) => {
              targetPath = "";
              preview = null;
              previewError = message;
            }}
          />
        {:else}
          <div
            class="text-muted-foreground flex min-h-48 items-center justify-center rounded-md border border-dashed p-4 text-center text-[13px]"
          >
            {m.projects_no_nodes()}
          </div>
        {/if}
      </div>

      <div class="bg-background flex min-h-48 flex-col gap-2 rounded-md border p-3 text-[13px]">
        {#if previewLoading}
          <div class="text-muted-foreground flex flex-1 items-center justify-center gap-2">
            <LoaderCircleIcon class="size-4 animate-spin" />
            {m.projects_inspecting()}
          </div>
        {:else if previewError !== null}
          <div
            class="text-destructive flex flex-1 flex-col items-center justify-center gap-2 text-center"
          >
            <AlertTriangleIcon class="size-5" />
            <span>{m.projects_inspect_failed()}</span>
            <span class="text-[11px] break-words opacity-80">{previewError}</span>
          </div>
        {:else if preview !== null}
          <span class="text-muted-foreground w-fit rounded-full border px-2 py-0.5 text-[11px]">
            {identityLabel()}
          </span>
          <h3 class="text-sm font-semibold">{preview.name}</h3>
          <code class="text-muted-foreground block truncate text-[11px]" title={effectiveProjectId}
            >{effectiveProjectId}</code
          >
          {#if preview.canonical_git_url}
            <p
              class="text-muted-foreground truncate font-mono text-[11px]"
              title={preview.canonical_git_url}
            >
              {preview.canonical_git_url}
            </p>
          {/if}
          {#if preview.relative_subpath}
            <p class="text-muted-foreground text-[11px]">
              {m.projects_monorepo_subpath({ path: preview.relative_subpath })}
            </p>
          {/if}
          {#if preview.is_worktree}
            <p class="text-muted-foreground flex items-center gap-1 text-[11px]">
              <GitBranchIcon class="size-3" />
              {m.projects_worktree_binding()}
            </p>
          {/if}
          <p
            class="text-muted-foreground truncate font-mono text-[11px]"
            title={preview.local_path}
          >
            {preview.local_path}
          </p>
          <div class="mt-auto flex flex-col gap-1 border-t pt-2 text-[11px] leading-relaxed">
            {#if alreadyAdded}
              <p class="text-muted-foreground">{m.projects_already_added()}</p>
            {:else if conflictingBinding}
              <p class="text-warn">
                {m.projects_binding_conflict({ project: conflictingBinding.project.name })}
              </p>
            {:else if existingProject}
              <p class="text-muted-foreground">
                {m.projects_existing_project_add_binding()}
              </p>
            {:else}
              <p class="text-muted-foreground">{m.projects_new_project_hint()}</p>
            {/if}
          </div>
        {:else}
          <div class="text-muted-foreground flex flex-1 items-center justify-center text-center">
            {m.projects_choose_folder_hint()}
          </div>
        {/if}
      </div>
    </div>

    <div class="border-t pt-3">
      <label class="text-muted-foreground flex items-center gap-2 text-[13px]">
        <input
          type="checkbox"
          class="border-input accent-primary size-3.5 rounded border"
          bind:checked={advanced}
        />
        {m.projects_advanced_custom_key()}
      </label>
      {#if advanced}
        <div class="mt-2 flex max-w-md flex-col gap-1">
          <Input
            bind:value={customKey}
            placeholder="my-project"
            class="h-8 font-mono text-[13px]"
            disabled={busy}
          />
          <p class="text-muted-foreground text-[11px]">
            {m.projects_custom_key_hint()}
          </p>
        </div>
      {/if}
    </div>

    {#if formError !== null}
      <p class="text-destructive text-[13px]">{formError}</p>
    {/if}

    <Dialog.Footer class="gap-2 sm:gap-2">
      <Button variant="outline" size="sm" onclick={() => (open = false)} disabled={busy}>
        {m.common_cancel()}
      </Button>
      <Button size="sm" disabled={!canSubmit && !alreadyAdded} onclick={() => void submit()}>
        {#if busy}
          <LoaderCircleIcon class="size-3.5 animate-spin" />
          {m.projects_adding()}
        {:else if alreadyAdded}
          {m.projects_use_this_project()}
        {:else if advanced}
          {m.projects_link_action()}
        {:else}
          {m.projects_add_action()}
        {/if}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
