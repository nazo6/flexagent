<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import NodeStatusBadge from "#lib/components/NodeStatusBadge.svelte";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import { agentOptionLabel, agentsAvailableOn } from "#lib/agents.ts";
  import type {
    AgentSummary,
    ProvisionerSummary,
    ProvisionerTestResponse,
  } from "#lib/generated/bindings.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import {
    buildPathCandidates,
    pickDefaultCandidate,
    pickDefaultNode,
    type PathCandidate,
  } from "#lib/new-session.ts";
  import {
    loadNewSessionPrefs,
    loadSessionConfig,
    saveNewSessionPrefs,
    type SessionModePreference,
  } from "#lib/session-prefs.ts";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { toast } from "svelte-sonner";
  import { onMount, untrack } from "svelte";
  import BotIcon from "@lucide/svelte/icons/bot";
  import BoxIcon from "@lucide/svelte/icons/box";
  import CheckCircle2Icon from "@lucide/svelte/icons/check-circle-2";
  import CornerDownLeftIcon from "@lucide/svelte/icons/corner-down-left";
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PlayIcon from "@lucide/svelte/icons/play";
  import ServerIcon from "@lucide/svelte/icons/server";
  import Settings2Icon from "@lucide/svelte/icons/settings-2";
  import SparklesIcon from "@lucide/svelte/icons/sparkles";
  import XCircleIcon from "@lucide/svelte/icons/x-circle";
  import XIcon from "@lucide/svelte/icons/x";

  type PathMode = "existing" | "new_worktree";
  type RunTarget = "node" | "provisioner";

  let selectedProjectId = $state("");
  let nodeId = $state("");
  let runTarget = $state<RunTarget>("node");
  let provisionerName = $state("");
  const provisioners = $state<ProvisionerSummary[]>([]);
  let agentId = $state("");
  /** Enter the ID directly even when a catalog is available. */
  let manualAgent = $state(false);
  /** ACP Registry catalog + per-node availability. */
  let catalog = $state<AgentSummary[]>([]);
  let catalogLoading = $state(true);
  let catalogError = $state<string | null>(null);
  let provisionerLoadError = $state<string | null>(null);
  let pathMode = $state<PathMode>("existing");
  let localPath = $state("");
  let branch = $state("");
  let baseBranch = $state("");
  let newPath = $state("");
  let promptText = $state("");
  let busy = $state(false);
  let showAdvanced = $state(false);
  let browserOpen = $state(false);
  let newWorktreeBrowserOpen = $state(false);
  let useCustomPath = $state(false);

  // Default resolution control: track the first application of URL params and node selection
  let nodeTouched = $state(false);
  let urlProjectApplied = $state(false);
  let urlNodeApplied = $state(false);
  let urlAgentApplied = $state(false);
  let lastAutoPathKey = "";

  // Per-device memory (scoped to a project): tracked so nothing is saved before restoring
  let prefsProjectId = "";
  let agentTouched = $state(false);
  let modeTouched = $state(false);

  // Fork inheritance
  let forkSessionId = $state("");
  let forkNodeSeq = $state<number | null>(null);

  // Launch options
  let sessionMode = $state<SessionModePreference>("default");
  let extraArgsText = $state("");

  // Provisioner connectivity test
  let testBusy = $state(false);
  let testResult = $state<ProvisionerTestResponse | null>(null);
  let testDialogOpen = $state(false);

  const projects = $derived(sync.projects);
  const selectedProject = $derived(
    projects.find((p) => p.project_id === selectedProjectId) ?? projects[0] ?? null,
  );

  const nodes = $derived(sync.nodes);
  const selectedNode = $derived(nodes.find((n) => n.node_id === nodeId) ?? null);
  // Agents that can actually be launched on this node (based on the catalog's
  // installed_nodes; npx/uvx distributed agents are included only once enabled
  // via "add from catalog")
  const availableAgents = $derived(agentsAvailableOn(catalog, nodeId));
  const availableAgentIds = $derived(availableAgents.map((agent) => agent.id));

  /** Loads provisioners and the agent catalog (retryable from the UI). */
  async function loadOptions(): Promise<void> {
    catalogLoading = true;
    try {
      const list = await sync.connection.client.provisioners();
      provisioners.splice(0, provisioners.length, ...list);
      provisionerLoadError = null;
      if (list.length > 0 && provisionerName === "") provisionerName = list[0].name;
    } catch (error) {
      // A local node has no provisioners, so treat this as a warning rather than
      // a hard failure; the user can still start resident-node sessions.
      provisionerLoadError = error instanceof Error ? error.message : String(error);
      problems.report("load provisioners", error, "warning");
    }
    // You can keep going by entering an ID directly even if the agent list cannot be fetched
    try {
      catalog = await sync.connection.client.agents();
      catalogError = null;
    } catch (error) {
      catalog = [];
      catalogError = error instanceof Error ? error.message : String(error);
      problems.report("load agents", error);
    } finally {
      catalogLoading = false;
    }
  }

  // Ephemeral VM provisioner definitions (only available when connected to the central server)
  onMount(() => {
    void loadOptions();
  });

  const isProvisionerMode = $derived(runTarget === "provisioner");
  const selectedProvisioner = $derived(
    provisioners.find((p) => p.name === provisionerName) ?? provisioners[0] ?? null,
  );

  // The URL `project` is applied only once (it must not overwrite later manual selections)
  $effect(() => {
    if (urlProjectApplied || projects.length === 0) return;
    const urlProject = page.url.searchParams.get("project");
    selectedProjectId =
      urlProject && projects.some((p) => p.project_id === urlProject)
        ? urlProject
        : selectedProjectId || projects[0].project_id;
    urlProjectApplied = true;
  });

  // Default to an online node with run history for the project
  // (stop following once the user has chosen a node)
  $effect(() => {
    if (nodeTouched || nodes.length === 0) return;
    const defaultNode = untrack(() => pickDefaultNode(nodes, selectedProject, sync.sessions));
    if (defaultNode !== "" && defaultNode !== nodeId) nodeId = defaultNode;
  });

  // URL `node` / `path` (deep link) are applied only once
  $effect(() => {
    if (urlNodeApplied || nodes.length === 0) return;
    const urlNode = page.url.searchParams.get("node");
    if (urlNode && nodes.some((n) => n.node_id === urlNode)) {
      nodeId = urlNode;
      nodeTouched = true;
    }
    const urlPath = page.url.searchParams.get("path");
    if (urlPath) {
      localPath = urlPath;
      useCustomPath = true;
    }
    urlNodeApplied = true;
  });

  // Resolve working directory candidates (bindings + recent sessions) in last-used order
  const pathCandidates = $derived.by(() =>
    buildPathCandidates(selectedProject, nodeId, sync.sessions),
  );
  const selectedCandidate = $derived(
    pathCandidates.find((candidate) => candidate.path === localPath) ?? null,
  );
  // Alternative nodes when this node has no candidates (another node with bindings)
  const otherNodesWithCandidates = $derived(
    nodes.filter(
      (node) =>
        node.node_id !== nodeId &&
        buildPathCandidates(selectedProject, node.node_id, sync.sessions).length > 0,
    ),
  );

  // Default candidate selection: update only when the project × node changes or the
  // current value disappears from the candidates (keep the selection when the
  // candidate order shifts due to session updates)
  $effect(() => {
    if (isProvisionerMode || useCustomPath) return;
    const key = `${selectedProject?.project_id ?? ""}|${nodeId}`;
    const containsCurrent = pathCandidates.some((candidate) => candidate.path === localPath);
    if (
      key === lastAutoPathKey &&
      (containsCurrent || (localPath === "" && pathCandidates.length === 0))
    ) {
      return;
    }
    lastAutoPathKey = key;
    if (!containsCurrent) localPath = pickDefaultCandidate(pathCandidates)?.path ?? "";
  });

  const effectiveAgent = $derived(agentId.trim());

  // Default agent: pick the first one usable on this node when nothing is selected
  $effect(() => {
    if (agentId !== "" || isProvisionerMode) return;
    const first = availableAgentIds[0];
    if (first !== undefined) agentId = first;
  });

  const canSubmit = $derived(
    !busy &&
      selectedProject !== null &&
      effectiveAgent !== "" &&
      (isProvisionerMode
        ? selectedProvisioner !== null
        : nodeId !== "" && (pathMode === "existing" ? localPath !== "" : branch.trim() !== "")),
  );

  // Explain to the user why submit is unavailable (avoid a silent disabled state)
  const pathMissing = $derived(
    !isProvisionerMode &&
      (pathMode === "existing" ? localPath.trim() === "" : branch.trim() === ""),
  );
  const pathRequiredHint = $derived(
    pathMode === "existing"
      ? m.new_session_path_required_existing()
      : m.new_session_path_required_branch(),
  );

  // URL `fork_session` / `agent` are applied only once
  $effect(() => {
    if (urlAgentApplied || nodes.length === 0) return;
    const forkSession = page.url.searchParams.get("fork_session");
    if (forkSession) {
      forkSessionId = forkSession;
      const forkSeqStr = page.url.searchParams.get("fork_seq");
      forkNodeSeq = forkSeqStr ? Number.parseInt(forkSeqStr, 10) : null;
    }
    const agentParam = page.url.searchParams.get("agent");
    if (agentParam) {
      agentId = agentParam;
      // Show IDs missing from the list in the input field so they are visible
      manualAgent = !availableAgentIds.includes(agentParam);
      // Deep links take precedence over memory
      agentTouched = true;
    }
    urlAgentApplied = true;
  });

  // Per-device memory: restore the previous launch settings when the project changes
  // (restoration must not overwrite items the user explicitly changed)
  $effect(() => {
    const project = selectedProject;
    if (!project) return;
    if (project.project_id !== prefsProjectId) {
      prefsProjectId = project.project_id;
      agentTouched = false;
      modeTouched = false;
    }
    const prefs = untrack(() => loadNewSessionPrefs(project.project_id));
    if (!prefs) return;
    if (!modeTouched && prefs.mode !== "default") sessionMode = prefs.mode;
    if (!agentTouched && prefs.agent !== "" && availableAgentIds.includes(prefs.agent)) {
      agentId = prefs.agent;
    }
  });

  // Persist changes to the launch settings on the local device
  // (`prefsProjectId` must match so nothing is saved before restoring)
  $effect(() => {
    void agentId;
    void sessionMode;
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "" || projectId !== prefsProjectId || isProvisionerMode) return;
    persistPrefs();
  });

  async function runProvisionerTest(name: string) {
    if (testBusy || !name) return;
    testBusy = true;
    testResult = null;
    testDialogOpen = true;
    try {
      const result = await sync.connection.client.testProvisioner(name);
      testResult = result;
      if (result.ok) {
        toast.success(m.new_session_provisioner_test_success({ name }));
      } else {
        toast.error(
          m.new_session_provisioner_test_failed({
            name,
            error: result.error ?? m.new_session_unknown_error(),
          }),
        );
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      testResult = {
        name,
        ok: false,
        node_id: null,
        log_lines: [],
        error: error instanceof Error ? error.message : String(error),
      };
    } finally {
      testBusy = false;
    }
  }

  async function handleStartSession(initialPrompt?: string) {
    if (!canSubmit || !selectedProject) return;
    busy = true;
    const promptToSend = (initialPrompt ?? promptText).trim();
    const useProvisioner = isProvisionerMode && selectedProvisioner !== null;
    const extraArgs = extraArgsText.trim() ? extraArgsText.trim().split(/\s+/) : null;
    // Remember on the local device for the next new session
    persistPrefs();

    try {
      const response = await sync.connection.client.createSession({
        command_id: crypto.randomUUID(),
        project_id: selectedProject.project_id,
        node_id: useProvisioner ? null : nodeId,
        provisioner: useProvisioner ? selectedProvisioner.name : null,
        local_path: useProvisioner || pathMode === "new_worktree" ? null : localPath,
        worktree: useProvisioner
          ? branch.trim() === ""
            ? null
            : { branch: branch.trim(), base_branch: null, new_path: null }
          : pathMode === "new_worktree"
            ? {
                branch: branch.trim(),
                base_branch: baseBranch.trim() === "" ? null : baseBranch.trim(),
                new_path: newPath.trim() === "" ? null : newPath.trim(),
              }
            : null,
        agent_id: effectiveAgent,
        initial_prompt: promptToSend === "" ? null : { text: promptToSend, attachments: [] },
        initial_config: Object.entries(loadSessionConfig(selectedProject.project_id)).map(
          ([key, value]) => ({ key, value }),
        ),
        mode: sessionMode === "default" ? null : sessionMode,
        extra_args: extraArgs,
        fork: forkSessionId
          ? {
              from_session_id: forkSessionId,
              from_node_seq: forkNodeSeq,
              restore_git_bundle_b64: null,
            }
          : null,
      });

      toast.success(
        useProvisioner
          ? m.new_session_ephemeral_started()
          : forkSessionId
            ? m.new_session_fork_started()
            : m.new_session_started(),
      );
      await sync.refreshSessions();
      await goto(`/sessions/${response.session_id}`);
    } catch (error) {
      // The form state (including the prompt) is intentionally kept so the
      // user can retry without retyping.
      problems.report("create session", error);
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      void handleStartSession();
    }
  }

  /** On project change: re-resolve the default node and default path. */
  function onProjectChange() {
    nodeTouched = false;
    useCustomPath = false;
  }

  /** On node change: re-resolve the default path since paths are per-node. */
  function onNodeChange() {
    nodeTouched = true;
    useCustomPath = false;
  }

  /** Save the previous launch settings to the local device (scoped to the project). */
  function persistPrefs() {
    const projectId = selectedProject?.project_id ?? "";
    // Do not remember ephemeral VMs because agent availability differs per node
    if (projectId === "" || projectId !== prefsProjectId || isProvisionerMode) return;
    saveNewSessionPrefs(projectId, {
      agent: effectiveAgent,
      mode: sessionMode,
    });
  }

  /** Display label for a working directory candidate. */
  function candidateLabel(candidate: PathCandidate): string {
    const parts: string[] = [];
    if (!candidate.exists) parts.push(m.new_session_path_missing_label());
    if (candidate.isWorktree) parts.push("[Worktree]");
    if (candidate.gitBranch) parts.push(`[${candidate.gitBranch}]`);
    parts.push(candidate.path);
    const suffix =
      candidate.source === "session" ? ` ${m.new_session_path_from_previous_session()}` : "";
    return `${parts.join(" ")} — ${formatRelativeTime(candidate.usedAt)}${suffix}`;
  }

  const suggestionPrompts = $derived([
    m.new_session_suggestion_1(),
    m.new_session_suggestion_2(),
    m.new_session_suggestion_3(),
    m.new_session_suggestion_4(),
  ]);
</script>

<div class="mx-auto flex h-full max-w-3xl flex-col justify-between px-4 py-6 md:py-10">
  <div class="flex flex-col gap-6">
    <div class="flex flex-col items-center gap-2 text-center">
      <div class="bg-primary/10 text-primary flex size-12 items-center justify-center rounded-2xl">
        <SparklesIcon class="size-6" />
      </div>
      <h1 class="text-2xl font-bold tracking-tight">{m.new_session_title()}</h1>
      <p class="text-muted-foreground text-sm max-w-md">
        {m.new_session_description()}
      </p>
    </div>

    {#if forkSessionId}
      <div
        class="bg-primary/10 border-primary/20 flex items-center justify-between rounded-xl border p-3 text-xs"
      >
        <div class="flex items-center gap-2">
          <GitForkIcon class="text-primary size-4 shrink-0" />
          <span>
            <code class="font-mono bg-background/50 px-1 py-0.5 rounded"
              >{forkSessionId.slice(0, 8)}</code
            >
            {#if forkNodeSeq !== null}
              {m.new_session_fork_banner_seq({ seq: forkNodeSeq })}
            {/if}
            — {m.new_session_fork_banner_description()}
          </span>
        </div>
        <Button
          variant="ghost"
          size="sm"
          class="h-6 px-2 text-xs"
          onclick={() => {
            forkSessionId = "";
            forkNodeSeq = null;
          }}
        >
          <XIcon class="size-3.5 mr-1" />
          {m.new_session_fork_clear()}
        </Button>
      </div>
    {/if}

    <div class="bg-card/70 rounded-xl border p-3.5 shadow-xs flex flex-col gap-3">
      <div class="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            <FolderGit2Icon class="size-3.5" />
            {m.new_session_project()}
          </Label>
          {#if projects.length > 0}
            <select
              bind:value={selectedProjectId}
              onchange={onProjectChange}
              class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
            >
              {#each projects as project (project.project_id)}
                <option value={project.project_id}>{project.name}</option>
              {/each}
            </select>
          {:else if sync.projectsLoading}
            <span class="text-muted-foreground flex h-8 items-center text-xs">
              {m.common_loading()}
            </span>
          {:else if sync.projectsError !== null}
            <div class="flex h-8 items-center gap-1.5 text-xs">
              <span class="text-destructive truncate" title={sync.projectsError}>
                {m.common_load_failed()}
              </span>
              <Button variant="ghost" size="xs" onclick={() => void sync.refreshProjects()}>
                {m.common_retry()}
              </Button>
            </div>
          {:else}
            <a
              href="/projects"
              class="border-input bg-muted/40 hover:bg-muted text-muted-foreground flex h-8 items-center justify-center rounded-md border text-xs"
            >
              {m.new_session_register_project()}
            </a>
          {/if}
        </div>

        <div class="flex flex-col gap-1">
          <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
            {#if isProvisionerMode}
              <BoxIcon class="size-3.5" />
              {m.new_session_provisioner()}
            {:else}
              <ServerIcon class="size-3.5" />
              {m.new_session_node()}
            {/if}
          </Label>
          {#if isProvisionerMode}
            <div class="flex items-center gap-1.5">
              {#if provisioners.length > 0}
                <select
                  bind:value={provisionerName}
                  class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1 flex-1 min-w-0"
                >
                  {#each provisioners as provisioner (provisioner.name)}
                    <option value={provisioner.name}>
                      {provisioner.description ?? provisioner.name}
                    </option>
                  {/each}
                </select>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 px-2 text-xs shrink-0"
                  disabled={!provisionerName || testBusy}
                  onclick={() => runProvisionerTest(provisionerName)}
                  title={m.new_session_provisioner_test_button_title()}
                >
                  {#if testBusy}
                    <LoaderCircleIcon class="size-3.5 animate-spin" />
                  {:else}
                    {m.new_session_test()}
                  {/if}
                </Button>
              {:else if provisionerLoadError !== null}
                <span class="text-destructive text-xs leading-8">
                  {m.common_load_failed()}
                  {provisionerLoadError}
                </span>
              {:else}
                <span class="text-muted-foreground text-xs leading-8">
                  {m.new_session_provisioner_undefined()}
                </span>
              {/if}
            </div>
          {:else}
            <select
              bind:value={nodeId}
              onchange={onNodeChange}
              class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
            >
              {#each nodes as node (node.node_id)}
                {@const avail = getNodeAvailability(node)}
                <option value={node.node_id}>
                  {node.name} ({avail.statusText})
                </option>
              {/each}
            </select>
          {/if}
        </div>

        <div class="flex flex-col gap-1">
          <div class="flex items-center gap-1.5">
            <Label class="text-muted-foreground text-xs flex items-center gap-1.5">
              <BotIcon class="size-3.5" />
              {m.new_session_agent()}
            </Label>
            {#if !isProvisionerMode && availableAgents.length > 0}
              <button
                type="button"
                class="text-muted-foreground hover:text-foreground ml-auto text-[10px] underline"
                onclick={() => (manualAgent = !manualAgent)}
              >
                {manualAgent ? m.new_session_choose_from_list() : m.new_session_enter_id()}
              </button>
            {/if}
          </div>
          {#if !isProvisionerMode && availableAgents.length > 0 && !manualAgent}
            <select
              bind:value={agentId}
              class="border-input bg-background focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
            >
              {#if agentId !== "" && !availableAgentIds.includes(agentId)}
                <option value={agentId}
                  >{m.new_session_agent_not_in_list({ agent: agentId })}</option
                >
              {/if}
              {#each availableAgents as agent (agent.id)}
                <option value={agent.id}>{agentOptionLabel(agent)}</option>
              {/each}
            </select>
          {:else}
            <Input
              bind:value={agentId}
              placeholder={m.new_session_agent_id_placeholder()}
              class="h-8 text-xs"
            />
          {/if}
          {#if !isProvisionerMode}
            {#if catalogError !== null}
              <div class="flex items-center gap-1.5 text-[10px]">
                <span class="text-destructive">
                  {m.common_load_failed()}
                  {catalogError}
                </span>
                <Button type="button" variant="ghost" size="xs" onclick={() => void loadOptions()}>
                  {m.common_retry()}
                </Button>
              </div>
            {:else if catalogLoading}
              <span class="text-muted-foreground text-[10px]">{m.common_loading()}</span>
            {/if}
            <a
              href="/settings?tab=agents"
              class="text-muted-foreground hover:text-foreground text-[10px] underline"
            >
              {availableAgents.length === 0
                ? m.new_session_no_agents()
                : m.new_session_manage_agents()}
            </a>
          {/if}
        </div>
      </div>

      <div class="flex items-center gap-1.5 text-xs">
        <span class="text-muted-foreground">{m.new_session_run_environment()}</span>
        <button
          type="button"
          class="rounded-full border px-2.5 py-0.5 transition-colors {runTarget === 'node'
            ? 'bg-primary text-primary-foreground border-primary'
            : 'text-muted-foreground hover:bg-muted'}"
          onclick={() => (runTarget = "node")}
        >
          {m.new_session_resident_node()}
        </button>
        <button
          type="button"
          class="rounded-full border px-2.5 py-0.5 transition-colors {isProvisionerMode
            ? 'bg-primary text-primary-foreground border-primary'
            : 'text-muted-foreground hover:bg-muted'}"
          onclick={() => (runTarget = "provisioner")}
          disabled={provisioners.length === 0}
          title={provisioners.length === 0
            ? m.new_session_provisioner_config_hint()
            : m.new_session_ephemeral_hint()}
        >
          {m.new_session_ephemeral()}
        </button>
        {#if isProvisionerMode}
          <span class="text-muted-foreground">
            — {m.new_session_ephemeral_note()}
          </span>
        {/if}
      </div>

      {#if !isProvisionerMode && projects.length > 0}
        <div class="flex flex-col gap-1.5 border-t pt-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <Label class="text-muted-foreground flex items-center gap-1.5 text-xs">
              <FolderGit2Icon class="size-3.5" />
              {m.new_session_working_directory()}
              <span class="text-destructive">*</span>
            </Label>
            <div class="flex items-center gap-1">
              <button
                type="button"
                class="rounded-full border px-2.5 py-0.5 text-xs transition-colors {pathMode ===
                'existing'
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'text-muted-foreground hover:bg-muted'}"
                onclick={() => (pathMode = "existing")}
              >
                {m.new_session_existing()}
              </button>
              <button
                type="button"
                class="rounded-full border px-2.5 py-0.5 text-xs transition-colors {pathMode ===
                'new_worktree'
                  ? 'bg-primary text-primary-foreground border-primary'
                  : 'text-muted-foreground hover:bg-muted'}"
                onclick={() => (pathMode = "new_worktree")}
              >
                {m.new_session_new_worktree()}
              </button>
            </div>
          </div>

          {#if pathMode === "existing"}
            <div class="flex items-center gap-1.5">
              {#if pathCandidates.length > 0 && !useCustomPath}
                <select
                  bind:value={localPath}
                  aria-invalid={localPath.trim() === ""}
                  class="border-input bg-background focus-visible:ring-ring/40 h-8 min-w-0 flex-1 rounded-md border px-2 text-xs outline-none focus-visible:ring-1 {localPath.trim() ===
                  ''
                    ? 'border-destructive'
                    : ''}"
                >
                  {#each pathCandidates as candidate (candidate.path)}
                    <option value={candidate.path}>{candidateLabel(candidate)}</option>
                  {/each}
                </select>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                  onclick={() => {
                    useCustomPath = true;
                    browserOpen = true;
                  }}
                  title={m.new_session_browse_other_dir()}
                >
                  <FolderOpenIcon class="size-3.5" />
                  <span>{m.new_session_browse()}</span>
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  class="text-muted-foreground hover:text-foreground h-8 shrink-0 px-2 text-xs"
                  onclick={() => (useCustomPath = true)}
                  title={m.new_session_enter_path_title()}
                >
                  {m.new_session_enter_path()}
                </Button>
              {:else}
                <Input
                  bind:value={localPath}
                  aria-invalid={localPath.trim() === ""}
                  placeholder={m.new_session_path_placeholder()}
                  class="h-8 min-w-0 flex-1 font-mono text-xs"
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                  disabled={!nodeId}
                  onclick={() => (browserOpen = true)}
                  title={m.new_session_browse_title()}
                >
                  <FolderOpenIcon class="size-3.5" />
                  <span>{m.new_session_browse()}</span>
                </Button>
                {#if pathCandidates.length > 0}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    class="text-muted-foreground hover:text-foreground h-8 shrink-0 px-2 text-xs"
                    onclick={() => {
                      useCustomPath = false;
                      localPath = pickDefaultCandidate(pathCandidates)?.path ?? "";
                    }}
                    title={m.new_session_back_to_list_title()}
                  >
                    {m.new_session_back_to_list()}
                  </Button>
                {/if}
              {/if}
            </div>

            {#if selectedCandidate}
              <p class="text-muted-foreground text-[11px]">
                {selectedCandidate.source === "binding"
                  ? m.new_session_source_binding()
                  : m.new_session_source_session()}
                — {m.new_session_last_used({
                  time: formatRelativeTime(selectedCandidate.usedAt),
                })}
                {#if selectedCandidate.isWorktree}
                  — Worktree
                {/if}
                {#if selectedCandidate.gitBranch}
                  — {selectedCandidate.gitBranch}
                {/if}
              </p>
            {:else if localPath.trim() !== ""}
              <p class="text-muted-foreground text-[11px]">
                {m.new_session_unregistered_path()}
              </p>
            {/if}

            {#if selectedCandidate && !selectedCandidate.exists}
              <p class="text-destructive text-[11px]">
                {m.new_session_path_not_exists()}
              </p>
            {/if}

            {#if pathCandidates.length === 0 && !(useCustomPath && localPath.trim() !== "")}
              <div
                class="flex flex-col gap-1.5 rounded-md border border-amber-500/40 bg-amber-500/10 p-2.5 text-[11px]"
              >
                <span>
                  {m.new_session_no_candidate({
                    project: selectedProject?.name ?? m.new_session_this_project(),
                  })}
                </span>
                <div class="flex flex-wrap items-center gap-1.5">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    class="h-7 px-2 text-[11px]"
                    onclick={() => {
                      useCustomPath = true;
                      browserOpen = true;
                    }}
                  >
                    {m.new_session_select_folder()}
                  </Button>
                  <a
                    href="/projects"
                    class="text-muted-foreground hover:text-foreground underline underline-offset-2"
                  >
                    {m.new_session_go_to_project_settings()}
                  </a>
                  {#each otherNodesWithCandidates as otherNode (otherNode.node_id)}
                    <button
                      type="button"
                      class="text-muted-foreground hover:text-foreground underline underline-offset-2"
                      onclick={() => {
                        nodeId = otherNode.node_id;
                        nodeTouched = true;
                      }}
                    >
                      {m.new_session_registered_on_node({ node: otherNode.name })}
                    </button>
                  {/each}
                </div>
              </div>
            {/if}
          {:else}
            <Input
              bind:value={branch}
              aria-invalid={branch.trim() === ""}
              placeholder="feat/new-task"
              class="h-8 text-xs"
            />
            <p class="text-muted-foreground text-[11px]">
              {m.new_session_worktree_description()}
            </p>
          {/if}
        </div>
      {/if}

      <div>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
          onclick={() => (showAdvanced = !showAdvanced)}
        >
          <Settings2Icon class="size-3" />
          <span>{showAdvanced ? m.new_session_hide_advanced() : m.new_session_advanced()}</span>
        </button>

        {#if showAdvanced}
          <div class="border-t pt-3 mt-2 flex flex-col gap-3">
            {#if isProvisionerMode}
              <div class="flex flex-col gap-1">
                <Label class="text-xs">{m.new_session_clone_branch()}</Label>
                <Input
                  bind:value={branch}
                  placeholder={m.new_session_clone_branch_placeholder()}
                  class="h-8 text-xs"
                />
              </div>
            {:else if pathMode === "new_worktree"}
              <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <div class="flex flex-col gap-1">
                  <Label class="text-xs">{m.new_session_base_branch()}</Label>
                  <Input bind:value={baseBranch} placeholder="main" class="h-8 text-xs" />
                </div>
                <div class="flex flex-col gap-1">
                  <Label class="text-xs">{m.new_session_destination_path()}</Label>
                  <div class="flex items-center gap-1.5">
                    <Input
                      bind:value={newPath}
                      placeholder={m.new_session_destination_placeholder()}
                      class="h-8 text-xs font-mono flex-1 min-w-0"
                    />
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      class="h-8 px-2.5 text-xs shrink-0 gap-1.5"
                      disabled={!nodeId}
                      onclick={() => (newWorktreeBrowserOpen = true)}
                      title={m.new_session_browse_destination_title()}
                    >
                      <FolderOpenIcon class="size-3.5" />
                      <span>{m.new_session_browse()}</span>
                    </Button>
                  </div>
                </div>
              </div>
            {/if}

            <div
              class="{isProvisionerMode || pathMode === 'new_worktree'
                ? 'border-t pt-2.5 '
                : ''}grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs"
            >
              <div class="flex flex-col gap-1">
                <Label class="text-xs">{m.new_session_initial_mode()}</Label>
                <select
                  bind:value={sessionMode}
                  class="border-input bg-background h-8 rounded-md border px-2 text-xs outline-none focus-visible:ring-1"
                >
                  <option value="default">{m.new_session_mode_default()}</option>
                  <option value="code">{m.new_session_mode_code()}</option>
                  <option value="plan">{m.new_session_mode_plan()}</option>
                </select>
              </div>

              <div class="flex flex-col gap-1">
                <Label class="text-xs">{m.new_session_extra_args()}</Label>
                <Input
                  bind:value={extraArgsText}
                  placeholder={m.new_session_extra_args_placeholder()}
                  class="h-8 text-xs font-mono"
                />
              </div>
            </div>
          </div>
        {/if}
      </div>
    </div>

    <div
      class="bg-card rounded-2xl border shadow-sm p-3 flex flex-col gap-2 focus-within:border-primary/50 transition-colors"
    >
      <textarea
        bind:value={promptText}
        onkeydown={onKeydown}
        rows="4"
        placeholder={m.new_session_prompt_placeholder()}
        class="bg-transparent placeholder:text-muted-foreground w-full resize-none text-sm outline-none leading-relaxed"
        disabled={busy}></textarea>

      <div class="flex items-center justify-between border-t pt-2.5">
        <div class="flex items-center gap-2">
          {#if isProvisionerMode}
            {#if selectedProvisioner}
              <span class="text-muted-foreground text-xs">
                {selectedProvisioner.name}
                {#if selectedProvisioner.idle_timeout_secs}
                  — {m.new_session_idle_discard({
                    minutes: Math.round(selectedProvisioner.idle_timeout_secs / 60),
                  })}
                {/if}
              </span>
            {/if}
          {:else if selectedNode}
            <NodeStatusBadge node={selectedNode} showNodeName={true} />
          {/if}
        </div>

        <div class="flex items-center gap-2">
          {#if pathMissing}
            <span class="text-destructive text-xs">{pathRequiredHint}</span>
          {:else if effectiveAgent === ""}
            <span class="text-destructive text-xs">{m.new_session_agent_required()}</span>
          {/if}
          <Button
            variant="ghost"
            size="sm"
            class="text-xs h-8 text-muted-foreground hover:text-foreground"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession("")}
            title={pathMissing ? pathRequiredHint : m.new_session_start_empty_title()}
          >
            {m.new_session_start_empty()}
          </Button>

          <Button
            size="sm"
            class="gap-1.5 h-8 text-xs font-medium"
            disabled={!canSubmit || busy}
            onclick={() => handleStartSession()}
            title={pathMissing ? pathRequiredHint : m.new_session_start_title()}
          >
            {#if busy}
              <span>{m.new_session_starting()}</span>
            {:else}
              <PlayIcon class="size-3.5 fill-current" />
              <span>{m.new_session_start()}</span>
              <kbd class="bg-primary-foreground/20 text-[10px] px-1 py-0.5 rounded font-mono">↵</kbd
              >
            {/if}
          </Button>
        </div>
      </div>
    </div>

    <div class="flex flex-col gap-2">
      <span class="text-muted-foreground text-xs">{m.new_session_suggestions()}</span>
      <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
        {#each suggestionPrompts as suggestion}
          <button
            type="button"
            class="hover:bg-accent/60 bg-muted/30 text-muted-foreground hover:text-foreground flex items-center justify-between rounded-lg border p-2.5 text-left text-xs transition-colors"
            onclick={() => {
              promptText = suggestion;
            }}
          >
            <span class="line-clamp-1">{suggestion}</span>
            <CornerDownLeftIcon class="size-3 shrink-0 opacity-60" />
          </button>
        {/each}
      </div>
    </div>
  </div>
</div>

<FolderBrowserDialog
  bind:open={browserOpen}
  {nodeId}
  initialPath={localPath}
  onSelect={(path) => {
    localPath = path;
    useCustomPath = true;
  }}
/>

<FolderBrowserDialog
  bind:open={newWorktreeBrowserOpen}
  {nodeId}
  initialPath={newPath || localPath}
  title={m.new_session_worktree_folder_title()}
  onSelect={(path) => {
    newPath = path;
  }}
/>

<Dialog.Root bind:open={testDialogOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{m.new_session_provisioner_test_title({ name: provisionerName })}</Dialog.Title>
      <Dialog.Description>
        {m.new_session_provisioner_test_description()}
      </Dialog.Description>
    </Dialog.Header>
    <div class="py-2 text-xs flex flex-col gap-3">
      {#if testBusy}
        <div class="flex flex-col items-center justify-center gap-2 py-8 text-muted-foreground">
          <LoaderCircleIcon class="size-6 animate-spin text-primary" />
          <span>{m.new_session_provisioner_starting()}</span>
        </div>
      {:else if testResult}
        <div class="flex items-center gap-2 font-medium">
          {#if testResult.ok}
            <CheckCircle2Icon class="size-5 text-green-500" />
            <span class="text-green-600 dark:text-green-400">{m.new_session_test_success()}</span>
            {#if testResult.node_id}
              <span class="text-muted-foreground font-mono text-[11px]">({testResult.node_id})</span
              >
            {/if}
          {:else}
            <XCircleIcon class="size-5 text-destructive" />
            <span class="text-destructive">{m.new_session_test_failed()}</span>
          {/if}
        </div>

        {#if testResult.error}
          <div
            class="bg-destructive/10 text-destructive border-destructive/20 rounded-md border p-2.5 font-mono text-[11px] whitespace-pre-wrap"
          >
            {testResult.error}
          </div>
        {/if}

        {#if testResult.log_lines && testResult.log_lines.length > 0}
          <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-[11px]">{m.new_session_run_log()}</span>
            <div
              class="bg-muted max-h-48 overflow-y-auto rounded-md p-2 font-mono text-[11px] leading-tight text-foreground whitespace-pre-wrap"
            >
              {testResult.log_lines.join("\n")}
            </div>
          </div>
        {/if}
      {/if}
    </div>
    <Dialog.Footer>
      <Button variant="outline" size="sm" onclick={() => (testDialogOpen = false)}
        >{m.common_close()}</Button
      >
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
