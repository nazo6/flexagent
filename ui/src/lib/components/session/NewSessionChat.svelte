<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import ComposerCard from "#lib/components/composer/ComposerCard.svelte";
  import ComposerSendButton from "#lib/components/composer/ComposerSendButton.svelte";
  import ComposerTextarea from "#lib/components/composer/ComposerTextarea.svelte";
  import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.svelte";
  import PickerItem from "#lib/components/composer/PickerItem.svelte";
  import PickerPill from "#lib/components/composer/PickerPill.svelte";
  import SessionSettingsDialog from "./SessionSettingsDialog.svelte";
  import { agentHint, agentsAvailableOn } from "#lib/agents.ts";
  import type {
    AgentCapabilitiesPreview,
    AgentSummary,
    ConfigOptionValue,
    InitialConfig,
    ProvisionerSummary,
    ProvisionerTestResponse,
  } from "#lib/generated/bindings.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import {
    isModelOption,
    orderedConfigOptions,
    resolveInitialConfig,
  } from "#lib/config-options.ts";
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
    saveSessionConfigOption,
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
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import Settings2Icon from "@lucide/svelte/icons/settings-2";
  import SparklesIcon from "@lucide/svelte/icons/sparkles";
  import ServerIcon from "@lucide/svelte/icons/server";
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
  /** Guards against stale responses when the node changes / the user retries. */
  let catalogGeneration = 0;
  let provisionerLoadError = $state<string | null>(null);
  let pathMode = $state<PathMode>("existing");
  let localPath = $state("");
  let branch = $state("");
  let baseBranch = $state("");
  let promptText = $state("");
  let busy = $state(false);
  let showSettings = $state(false);
  let browserOpen = $state(false);
  let useCustomPath = $state(false);

  // Picker popovers
  let projectOpen = $state(false);
  let directoryOpen = $state(false);
  let environmentOpen = $state(false);
  let agentOpen = $state(false);

  // Default resolution control: track the first application of URL params and node selection
  let nodeTouched = $state(false);
  let urlProjectApplied = $state(false);
  let urlNodeApplied = $state(false);
  let urlAgentApplied = $state(false);
  let lastAutoPathKey = "";

  // Per-device memory (scoped to a project): tracked so nothing is saved before restoring
  let prefsProjectId = "";
  /** Project whose saved preferences have been applied (empty until the catalog is ready). */
  let prefsResolvedProjectId = $state("");
  let agentTouched = $state(false);
  let modeTouched = $state(false);

  // Fork inheritance
  let forkSessionId = $state("");

  // Launch options
  let sessionMode = $state<SessionModePreference>("default");
  let extraArgsText = $state("");

  // Agent capability probe (models / modes)
  let capabilities = $state<AgentCapabilitiesPreview | null>(null);
  let capabilitiesKey = $state("");
  let capabilitiesLoading = $state(false);
  let capabilitiesError = $state<string | null>(null);
  let capabilitiesGeneration = 0;
  /** Key whose probe is currently rendered (cleared immediately on selection changes). */
  let lastProbeKey = "";
  /** Per-screen probe cache so re-selecting the same agent is instant. */
  const capabilitiesCache = new Map<string, AgentCapabilitiesPreview>();
  let selectedConfigValues = $state<Record<string, ConfigOptionValue>>({});

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
  const nodeAvailability = $derived(getNodeAvailability(selectedNode));
  // Agents that can actually be launched on this node (based on the catalog's
  // installed_nodes; npx/uvx distributed agents are included only once enabled
  // via "add from catalog")
  const availableAgents = $derived(agentsAvailableOn(catalog, nodeId));
  const availableAgentIds = $derived(availableAgents.map((agent) => agent.id));
  const selectedAgent = $derived(availableAgents.find((agent) => agent.id === agentId) ?? null);
  const effectiveAgent = $derived(agentId.trim());

  const isProvisionerMode = $derived(runTarget === "provisioner");
  const selectedProvisioner = $derived(
    provisioners.find((p) => p.name === provisionerName) ?? provisioners[0] ?? null,
  );

  /** Loads ephemeral VM provisioners (central server only; retryable from the UI). */
  async function loadProvisioners(): Promise<void> {
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
  }

  /**
   * Loads the agent catalog for one node.
   *
   * The central server rejects the unscoped `GET /api/v1/agents` (every catalog is
   * node-scoped), so always fetch through `/api/v1/nodes/{id}/agents`. You can keep going by
   * entering an ID directly even if the catalog cannot be fetched.
   */
  async function loadAgentCatalog(target: string): Promise<void> {
    if (target === "") return;
    // Only the newest request may publish (node switches / retries can overlap)
    const generation = ++catalogGeneration;
    catalogLoading = true;
    try {
      const list = await sync.connection.client.nodeAgents(target);
      if (generation !== catalogGeneration) return;
      catalog = list;
      catalogError = null;
    } catch (error) {
      if (generation !== catalogGeneration) return;
      catalog = [];
      catalogError = error instanceof Error ? error.message : String(error);
      problems.report("load agents", error);
    } finally {
      if (generation === catalogGeneration) catalogLoading = false;
    }
  }

  // Ephemeral VM provisioner definitions (only available when connected to the central server)
  onMount(() => {
    void loadProvisioners();
  });

  // The agent catalog is node-scoped: reload it when the target node changes.
  $effect(() => {
    const target = nodeId;
    if (target === "") {
      // No node to query (e.g. the hub has no connected nodes): stop the loading state.
      catalogLoading = false;
      return;
    }
    void loadAgentCatalog(target);
  });

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

  // Default agent: pick the first one usable on this node when nothing is selected.
  // Wait until the per-project preference has been restored so a stored agent is
  // not briefly replaced (and probed) before the preference is applied.
  $effect(() => {
    if (agentId !== "" || isProvisionerMode || selectedProject === null) return;
    if (prefsResolvedProjectId !== selectedProject.project_id) return;
    const first = availableAgentIds[0];
    if (first !== undefined) agentId = first;
  });

  // URL `fork_session` / `agent` are applied only once (after the node catalog is loaded, so
  // an agent from the link is recognized as a dropdown entry instead of a manual ID)
  $effect(() => {
    if (urlAgentApplied || nodes.length === 0 || catalogLoading) return;
    const forkSession = page.url.searchParams.get("fork_session");
    if (forkSession) {
      forkSessionId = forkSession;
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
    if (!project || catalogLoading) return;
    if (project.project_id !== prefsProjectId) {
      prefsProjectId = project.project_id;
      prefsResolvedProjectId = "";
      agentTouched = false;
      modeTouched = false;
    }
    const prefs = untrack(() => loadNewSessionPrefs(project.project_id));
    if (prefs) {
      if (!modeTouched && prefs.mode !== "default") sessionMode = prefs.mode;
      if (!agentTouched && prefs.agent !== "" && availableAgentIds.includes(prefs.agent)) {
        agentId = prefs.agent;
      }
    }
    prefsResolvedProjectId = project.project_id;
  });

  // Persist changes to the launch settings on the local device.
  //
  // Only explicit user changes are saved: the initial default agent / mode must
  // not overwrite the restored per-project preference before the catalog has
  // loaded (`agentTouched` / `modeTouched`).
  $effect(() => {
    void agentId;
    void sessionMode;
    if (!agentTouched && !modeTouched) return;
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "" || projectId !== prefsProjectId || isProvisionerMode) return;
    persistPrefs();
  });

  /** Working directory used to probe the agent for models. */
  const probePath = $derived(
    isProvisionerMode
      ? ""
      : pathMode === "existing"
        ? localPath
        : (pathCandidates.find((candidate) => candidate.exists)?.path ??
          pathCandidates[0]?.path ??
          ""),
  );

  /**
   * Model selection applied to the capability probe.
   *
   * Some agents (for example opencode) publish dependent options such as
   * `effort` only after a model is selected. Before the first probe the only
   * model key we know is the conventional `model` from the saved preferences.
   */
  function currentModelConfig(): InitialConfig[] {
    if (capabilities !== null) {
      const option = (capabilities.config_options ?? []).find((candidate) =>
        isModelOption(candidate),
      );
      if (option === undefined) return [];
      const value = selectedConfigValues[option.key];
      return value === undefined ? [] : [{ key: option.key, value }];
    }
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "") return [];
    const saved = loadSessionConfig(projectId)["model"];
    return saved === undefined ? [] : [{ key: "model", value: saved }];
  }

  /** UI cache key for one probe request (agent / path / initial config). */
  function probeCacheKey(key: string, config: InitialConfig[]): string {
    return `${key}\u0000${JSON.stringify(config)}`;
  }

  /** Stable key for the probe target (node / agent / working directory). */
  const probeKey = $derived(
    isProvisionerMode ||
      !nodeAvailability.isAvailable ||
      nodeId === "" ||
      effectiveAgent === "" ||
      probePath === ""
      ? ""
      : `${nodeId}\u0000${effectiveAgent}\u0000${probePath}`,
  );

  // Only show options that belong to the current selection. This prevents the
  // previous agent's model list from flashing while the next probe is running.
  const configOptions = $derived(
    capabilitiesKey === probeKey ? orderedConfigOptions(capabilities?.config_options ?? []) : [],
  );

  function sameConfig(left: InitialConfig[], right: InitialConfig[]): boolean {
    return JSON.stringify(left) === JSON.stringify(right);
  }

  function applyCapabilities(
    key: string,
    preview: AgentCapabilitiesPreview,
    probedConfig: InitialConfig[],
  ): void {
    capabilities = preview;
    capabilitiesKey = key;
    capabilitiesLoading = false;
    const projectId = selectedProject?.project_id ?? "";
    const saved = projectId === "" ? {} : loadSessionConfig(projectId);
    selectedConfigValues = resolveInitialConfig(preview.config_options ?? [], saved);

    // The probe may have used no model (first visit) or a remembered model that
    // is no longer available. Probe once more with the model actually selected
    // so dependent options such as effort are included.
    const selectedModelConfig = currentModelConfig();
    if (selectedModelConfig.length > 0 && !sameConfig(selectedModelConfig, probedConfig)) {
      void loadAgentCapabilities(false, selectedModelConfig);
    }
  }

  async function loadAgentCapabilities(
    force = false,
    config: InitialConfig[] = currentModelConfig(),
  ): Promise<void> {
    const key = probeKey;
    if (key === "") {
      capabilitiesGeneration += 1;
      capabilities = null;
      capabilitiesKey = "";
      capabilitiesLoading = false;
      capabilitiesError = null;
      selectedConfigValues = {};
      return;
    }
    const cacheKey = probeCacheKey(key, config);
    if (!force) {
      const cached = capabilitiesCache.get(cacheKey);
      if (cached) {
        applyCapabilities(key, cached, config);
        return;
      }
    }
    const generation = ++capabilitiesGeneration;
    capabilitiesLoading = true;
    capabilitiesError = null;
    try {
      const preview = await sync.connection.client.probeAgent(
        nodeId,
        effectiveAgent,
        probePath,
        force,
        config,
      );
      if (generation !== capabilitiesGeneration) return;
      capabilitiesCache.set(cacheKey, preview);
      applyCapabilities(key, preview, config);
    } catch (error) {
      if (generation !== capabilitiesGeneration) return;
      capabilities = null;
      capabilitiesKey = key;
      selectedConfigValues = {};
      capabilitiesError = error instanceof Error ? error.message : String(error);
      problems.report("probe agent", error, "warning");
    } finally {
      if (generation === capabilitiesGeneration) capabilitiesLoading = false;
    }
  }

  // Probe when the node / agent / working directory selection changes.
  $effect(() => {
    const key = probeKey;
    if (key === "") {
      lastProbeKey = "";
      capabilitiesGeneration += 1;
      capabilities = null;
      capabilitiesKey = "";
      capabilitiesLoading = false;
      capabilitiesError = null;
      selectedConfigValues = {};
      return;
    }
    if (key !== lastProbeKey) {
      lastProbeKey = key;
      // Drop the previous agent's/path's options immediately; the new probe
      // starts from a clean loading state.
      capabilities = null;
      capabilitiesKey = "";
      capabilitiesLoading = true;
      capabilitiesError = null;
      selectedConfigValues = {};
    }
    void loadAgentCapabilities(
      false,
      untrack(() => currentModelConfig()),
    );
  });

  function changeConfigOption(key: string, value: ConfigOptionValue): void {
    selectedConfigValues = { ...selectedConfigValues, [key]: value };
    const projectId = selectedProject?.project_id ?? "";
    if (projectId !== "") saveSessionConfigOption(projectId, key, value);

    // Selecting a model can reveal dependent options (for example opencode's
    // effort levels) that the previous probe did not return.
    const option = (capabilities?.config_options ?? []).find((candidate) => candidate.key === key);
    if (option !== undefined && isModelOption(option)) {
      void loadAgentCapabilities(true, [{ key, value }]);
    }
  }

  function initialConfig(): { key: string; value: ConfigOptionValue }[] {
    // Never send an option list that belongs to a different agent / path.
    if (probeKey !== "" && capabilitiesKey !== probeKey) return [];
    if (capabilities !== null) {
      return Object.entries(selectedConfigValues).map(([key, value]) => ({ key, value }));
    }
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "") return [];
    return Object.entries(loadSessionConfig(projectId)).map(([key, value]) => ({
      key,
      value,
    }));
  }

  const canSubmit = $derived(
    !busy &&
      selectedProject !== null &&
      effectiveAgent !== "" &&
      (isProvisionerMode
        ? selectedProvisioner !== null
        : nodeId !== "" &&
          nodeAvailability.isAvailable &&
          (pathMode === "existing" ? localPath !== "" : branch.trim() !== "")),
  );

  // Explain to the user why submit is unavailable (avoid a silent disabled state)
  const blockingHint = $derived.by(() => {
    // The project callout above the composer already explains these states.
    if (sync.projectsLoading || projects.length === 0 || sync.projectsError !== null) return "";
    if (!isProvisionerMode) {
      if (nodes.length === 0) return m.new_session_no_nodes();
      if (selectedNode !== null && !nodeAvailability.isAvailable) {
        return m.new_session_node_unavailable({
          node: selectedNode.name,
          status: nodeAvailability.statusText,
        });
      }
    } else if (selectedProvisioner === null) {
      return m.new_session_provisioner_undefined();
    }
    if (effectiveAgent === "") return m.new_session_agent_required();
    if (
      !isProvisionerMode &&
      (pathMode === "existing" ? localPath.trim() === "" : branch.trim() === "")
    ) {
      return pathMode === "existing"
        ? m.new_session_path_required_existing()
        : m.new_session_path_required_branch();
    }
    return "";
  });

  const startTitle = $derived(
    blockingHint !== ""
      ? blockingHint
      : promptText.trim() === ""
        ? m.new_session_start_empty_title()
        : m.new_session_start_title(),
  );

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }

  const projectLabel = $derived(selectedProject?.name ?? "");
  const directoryLabel = $derived(
    pathMode === "new_worktree"
      ? branch.trim() || m.new_session_new_worktree()
      : localPath.trim() !== ""
        ? pathBasename(localPath)
        : "",
  );
  const environmentLabel = $derived(
    isProvisionerMode
      ? (selectedProvisioner?.description ?? selectedProvisioner?.name ?? provisionerName)
      : (selectedNode?.name ?? ""),
  );
  const agentLabel = $derived(manualAgent ? effectiveAgent : (selectedAgent?.name ?? ""));

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
            : { branch: branch.trim(), base_branch: null }
          : pathMode === "new_worktree"
            ? {
                branch: branch.trim(),
                base_branch: baseBranch.trim() === "" ? null : baseBranch.trim(),
              }
            : null,
        agent_id: effectiveAgent,
        initial_prompt: promptToSend === "" ? null : { text: promptToSend, attachments: [] },
        initial_config: initialConfig(),
        mode: sessionMode === "default" ? null : sessionMode,
        extra_args: extraArgs,
        fork: forkSessionId
          ? {
              from_session_id: forkSessionId,
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
    if (event.isComposing || event.keyCode === 229) return;
    const startWithModifier = event.ctrlKey || event.metaKey;
    if (event.key === "Enter" && (startWithModifier || !event.shiftKey)) {
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

  function candidateDetail(candidate: PathCandidate): string {
    const parts: string[] = [];
    if (!candidate.exists) parts.push(m.new_session_path_missing_label());
    if (candidate.isWorktree) parts.push("Worktree");
    if (candidate.gitBranch) parts.push(candidate.gitBranch);
    parts.push(formatRelativeTime(candidate.usedAt));
    return parts.join(" · ");
  }

  function selectProject(projectId: string): void {
    selectedProjectId = projectId;
    onProjectChange();
    projectOpen = false;
  }

  function selectNode(id: string): void {
    runTarget = "node";
    nodeId = id;
    onNodeChange();
    environmentOpen = false;
  }

  function selectProvisioner(name: string): void {
    runTarget = "provisioner";
    provisionerName = name;
    environmentOpen = false;
  }

  function selectAgent(id: string): void {
    agentId = id;
    manualAgent = false;
    agentTouched = true;
    agentOpen = false;
  }

  function onManualAgentInput(): void {
    agentTouched = true;
  }

  function onSessionModeChange(): void {
    modeTouched = true;
  }

  function selectPath(path: string): void {
    localPath = path;
    useCustomPath = false;
    directoryOpen = false;
  }

  function resetPathToDefault(): void {
    useCustomPath = false;
    localPath = pickDefaultCandidate(pathCandidates)?.path ?? "";
  }

  function openFolderBrowser(): void {
    directoryOpen = false;
    browserOpen = true;
  }
</script>

{#snippet projectIcon()}
  <FolderGit2Icon class="text-muted-foreground size-3.5" />
{/snippet}

{#snippet directoryIcon()}
  <GitBranchIcon class="text-muted-foreground size-3.5" />
{/snippet}

{#snippet environmentIcon()}
  {#if isProvisionerMode}
    <BoxIcon class="text-muted-foreground size-3.5" />
  {:else}
    <span class="relative inline-flex">
      <ServerIcon class="text-muted-foreground size-3.5" />
      <span
        class="ring-background absolute -right-1 -bottom-0.5 size-1.5 rounded-full ring-1 {nodeAvailability.dotColorClass}"
      ></span>
    </span>
  {/if}
{/snippet}

{#snippet agentIcon()}
  <BotIcon class="text-muted-foreground size-3.5" />
{/snippet}

{#snippet modelIcon()}
  <SparklesIcon class="text-muted-foreground size-3.5" />
{/snippet}

<div
  class="mx-auto flex min-h-full w-full max-w-[680px] flex-col justify-center px-4 py-8 md:py-12"
>
  <div class="flex flex-col gap-5">
    <div class="flex flex-col items-center gap-2.5 text-center">
      <div class="bg-signal-soft text-signal flex size-11 items-center justify-center rounded-xl">
        <SparklesIcon class="size-5" />
      </div>
      <h1 class="text-2xl font-semibold tracking-tight">{m.new_session_title()}</h1>
      <p class="text-muted-foreground max-w-md text-ui">
        {m.new_session_description()}
      </p>
    </div>

    {#if forkSessionId}
      <div
        class="bg-signal-soft/70 border-signal/25 flex items-center justify-between gap-2 rounded-lg border p-3 text-ui"
      >
        <div class="flex min-w-0 items-center gap-2">
          <GitForkIcon class="text-primary size-4 shrink-0" />
          <span class="truncate">
            <code class="bg-background/50 rounded px-1 py-0.5 font-mono"
              >{forkSessionId.slice(0, 8)}</code
            >
            — {m.new_session_fork_banner_description()}
          </span>
        </div>
        <Button
          variant="ghost"
          size="sm"
          class="h-6 shrink-0 px-2 text-ui"
          onclick={() => {
            forkSessionId = "";
          }}
        >
          <XIcon class="mr-1 size-3.5" />
          {m.new_session_fork_clear()}
        </Button>
      </div>
    {/if}

    {#if sync.projectsLoading && projects.length === 0}
      <div
        class="bg-chip text-muted-foreground flex items-center justify-center gap-2 rounded-lg border border-border p-3 text-ui"
      >
        <LoaderCircleIcon class="size-3.5 animate-spin" />
        {m.common_loading()}
      </div>
    {:else if projects.length === 0 && sync.projectsError === null}
      <div
        class="border-warn/35 bg-warn-soft/60 flex flex-wrap items-center justify-between gap-2 rounded-lg border p-3 text-ui"
      >
        <span class="flex items-center gap-2">
          <FolderGit2Icon class="size-4 shrink-0" />
          {m.new_session_no_projects()}
        </span>
        <a
          href="/projects"
          class="bg-background hover:bg-muted rounded-md border px-2.5 py-1 text-ui font-medium"
        >
          {m.new_session_register_project()}
        </a>
      </div>
    {:else if sync.projectsError !== null}
      <div
        class="border-danger/30 bg-danger-soft/60 text-danger flex items-center justify-between gap-2 rounded-lg border p-3 text-ui"
      >
        <span class="truncate">{sync.projectsError}</span>
        <Button variant="ghost" size="xs" onclick={() => void sync.refreshProjects()}>
          {m.common_retry()}
        </Button>
      </div>
    {/if}

    <ComposerCard>
      {#snippet above()}
        <div class="flex flex-wrap items-center gap-1.5 px-1 pt-1">
          <PickerPill
            label={m.new_session_project()}
            value={projectLabel}
            icon={projectIcon}
            bind:open={projectOpen}
            disabled={projects.length === 0}
          >
            {#if projects.length > 0}
              <div class="max-h-72 overflow-y-auto p-1">
                {#each projects as project (project.project_id)}
                  <PickerItem
                    selected={project.project_id === selectedProject?.project_id}
                    onselect={() => selectProject(project.project_id)}
                  >
                    <span class="block truncate">{project.name}</span>
                  </PickerItem>
                {/each}
              </div>
              <div class="border-t p-1">
                <a
                  href="/projects"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-ui"
                >
                  {m.new_session_manage_projects()}
                </a>
              </div>
            {:else}
              <div class="p-1">
                <a
                  href="/projects"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-ui"
                >
                  {m.new_session_register_project()}
                </a>
              </div>
            {/if}
          </PickerPill>

          <PickerPill
            label={m.new_session_working_directory()}
            value={directoryLabel}
            icon={directoryIcon}
            contentClass="w-[min(24rem,calc(100vw-1.5rem))]"
            bind:open={directoryOpen}
            disabled={isProvisionerMode || selectedProject === null}
          >
            {#if isProvisionerMode}
              <p class="text-muted-foreground p-2 text-ui">{m.new_session_ephemeral_note()}</p>
            {:else if pathMode === "new_worktree"}
              <div class="flex flex-col gap-2 p-2">
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-2xs">{m.new_session_new_worktree()}</span>
                  <Input bind:value={branch} placeholder="feat/new-task" class="h-8 text-ui" />
                </div>
                <p class="text-muted-foreground text-2xs">
                  {m.new_session_worktree_description()}
                </p>
                <button
                  type="button"
                  class="text-muted-foreground hover:text-foreground self-start text-2xs underline"
                  onclick={() => {
                    pathMode = "existing";
                    directoryOpen = false;
                  }}
                >
                  {m.new_session_back_to_list()}
                </button>
              </div>
            {:else if useCustomPath}
              <div class="flex flex-col gap-2 p-2">
                <Input
                  bind:value={localPath}
                  placeholder={m.new_session_path_placeholder()}
                  class="h-8 font-mono text-ui"
                  autofocus
                />
                <div class="flex flex-wrap items-center gap-1.5">
                  <Button variant="outline" size="sm" onclick={openFolderBrowser}>
                    {m.new_session_browse()}
                  </Button>
                  {#if pathCandidates.length > 0}
                    <Button variant="ghost" size="sm" onclick={resetPathToDefault}>
                      {m.new_session_back_to_list()}
                    </Button>
                  {/if}
                </div>
                {#if localPath.trim() !== "" && selectedCandidate === null}
                  <p class="text-muted-foreground text-2xs">
                    {m.new_session_unregistered_path()}
                  </p>
                {/if}
                {#if selectedCandidate && !selectedCandidate.exists}
                  <p class="text-destructive text-2xs">{m.new_session_path_not_exists()}</p>
                {/if}
              </div>
            {:else if pathCandidates.length > 0}
              <div class="max-h-72 overflow-y-auto p-1">
                {#each pathCandidates as candidate (candidate.path)}
                  <PickerItem
                    selected={candidate.path === localPath}
                    onselect={() => selectPath(candidate.path)}
                  >
                    <span class="block truncate">{pathBasename(candidate.path)}</span>
                    <span
                      class="text-muted-foreground block truncate text-2xs {candidate.exists
                        ? ''
                        : 'text-destructive'}"
                      title={candidateLabel(candidate)}
                    >
                      {candidate.path} · {candidateDetail(candidate)}
                    </span>
                  </PickerItem>
                {/each}
              </div>
              <div class="flex flex-wrap items-center gap-1 border-t p-1">
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-2xs"
                  onclick={openFolderBrowser}
                >
                  {m.new_session_browse()}
                </button>
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-2xs"
                  onclick={() => (useCustomPath = true)}
                >
                  {m.new_session_enter_path()}
                </button>
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-2xs"
                  onclick={() => {
                    pathMode = "new_worktree";
                    directoryOpen = false;
                  }}
                >
                  {m.new_session_new_worktree()}
                </button>
              </div>
            {:else}
              <div class="flex flex-col gap-2 p-2">
                <p class="text-muted-foreground text-ui">
                  {m.new_session_no_candidate({
                    project: selectedProject?.name ?? m.new_session_this_project(),
                  })}
                </p>
                <div class="flex flex-wrap items-center gap-1.5">
                  <Button variant="outline" size="sm" onclick={openFolderBrowser}>
                    {m.new_session_select_folder()}
                  </Button>
                  <a
                    href="/projects"
                    class="text-muted-foreground hover:text-foreground text-2xs underline underline-offset-2"
                  >
                    {m.new_session_go_to_project_settings()}
                  </a>
                </div>
                {#each otherNodesWithCandidates as otherNode (otherNode.node_id)}
                  <button
                    type="button"
                    class="text-muted-foreground hover:text-foreground self-start text-2xs underline"
                    onclick={() => selectNode(otherNode.node_id)}
                  >
                    {m.new_session_registered_on_node({ node: otherNode.name })}
                  </button>
                {/each}
              </div>
            {/if}
          </PickerPill>

          <span class="flex-1"></span>

          <PickerPill
            label={m.new_session_environment()}
            value={environmentLabel}
            icon={environmentIcon}
            contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
            bind:open={environmentOpen}
            disabled={nodes.length === 0 && provisioners.length === 0}
          >
            <div class="max-h-80 overflow-y-auto p-1">
              <div
                class="text-muted-foreground px-2 py-1 text-2xs font-medium tracking-wide uppercase"
              >
                {m.new_session_node_section()}
              </div>
              {#if nodes.length === 0}
                <p class="text-muted-foreground px-2 py-1.5 text-ui">
                  {m.new_session_no_nodes()}
                </p>
              {:else}
                {#each nodes as node (node.node_id)}
                  {@const availability = getNodeAvailability(node)}
                  <PickerItem
                    selected={!isProvisionerMode && node.node_id === nodeId}
                    onselect={() => selectNode(node.node_id)}
                  >
                    <span class="flex items-center gap-1.5">
                      <span class="size-1.5 shrink-0 rounded-full {availability.dotColorClass}"
                      ></span>
                      <span class="truncate">{node.name}</span>
                    </span>
                    <span class="text-muted-foreground block truncate text-2xs">
                      {availability.statusText}
                    </span>
                  </PickerItem>
                {/each}
              {/if}

              <div class="border-t p-1 pt-2">
                <div
                  class="text-muted-foreground px-2 py-1 text-2xs font-medium tracking-wide uppercase"
                >
                  {m.new_session_ephemeral_section()}
                </div>
                {#if provisioners.length > 0}
                  {#each provisioners as provisioner (provisioner.name)}
                    <PickerItem
                      selected={isProvisionerMode && provisioner.name === provisionerName}
                      onselect={() => selectProvisioner(provisioner.name)}
                    >
                      <span class="block truncate">
                        {provisioner.description ?? provisioner.name}
                      </span>
                      <span class="text-muted-foreground block truncate text-2xs">
                        {provisioner.name}
                      </span>
                    </PickerItem>
                  {/each}
                {:else if provisionerLoadError !== null}
                  <div class="flex items-center gap-1.5 px-2 py-1 text-2xs">
                    <span class="text-destructive truncate">{m.common_load_failed()}</span>
                    <button
                      type="button"
                      class="text-muted-foreground hover:text-foreground underline"
                      onclick={() => void loadProvisioners()}
                    >
                      {m.common_retry()}
                    </button>
                  </div>
                {:else}
                  <p class="text-muted-foreground px-2 py-1 text-2xs">
                    {m.new_session_provisioner_undefined()}
                  </p>
                  <p class="text-muted-foreground px-2 pb-1 text-2xs">
                    {m.new_session_provisioner_config_hint()}
                  </p>
                {/if}
              </div>
            </div>
          </PickerPill>
        </div>
      {/snippet}

      <ComposerTextarea
        bind:value={promptText}
        onkeydown={onKeydown}
        rows={3}
        placeholder={m.new_session_prompt_placeholder()}
        disabled={busy}
      />

      {#snippet toolbar()}
        <PickerPill
          label={m.new_session_agent()}
          value={agentLabel}
          icon={agentIcon}
          contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
          bind:open={agentOpen}
          disabled={forkSessionId !== ""}
        >
          {#if catalogLoading}
            <p class="text-muted-foreground flex items-center gap-1.5 px-2 py-2 text-ui">
              <LoaderCircleIcon class="size-3.5 animate-spin" />
              {m.common_loading()}
            </p>
          {:else if catalogError !== null}
            <div class="flex flex-col gap-2 p-2">
              <p class="text-destructive break-words text-2xs">{catalogError}</p>
              <Button
                variant="outline"
                size="sm"
                class="self-start"
                onclick={() => void loadAgentCatalog(nodeId)}
              >
                {m.common_retry()}
              </Button>
            </div>
          {:else if availableAgents.length === 0}
            <p class="text-muted-foreground p-2 text-ui">{m.new_session_no_agents()}</p>
          {:else}
            <div class="max-h-72 overflow-y-auto p-1">
              {#each availableAgents as agent (agent.id)}
                <PickerItem
                  selected={agent.id === effectiveAgent}
                  onselect={() => selectAgent(agent.id)}
                >
                  <span class="block truncate">{agent.name}</span>
                  <span class="text-muted-foreground block truncate text-2xs">
                    {agentHint(agent)}
                  </span>
                </PickerItem>
              {/each}
            </div>
          {/if}
          <div class="border-t p-1">
            <a
              href="/settings?tab=agents"
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-ui"
            >
              {m.new_session_manage_agents()}
            </a>
          </div>
        </PickerPill>

        {#if isProvisionerMode}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_models_after_start()}
            icon={modelIcon}
            disabled
          />
        {:else if probeKey !== "" && (capabilitiesLoading || capabilitiesKey !== probeKey)}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_models_loading()}
            icon={modelIcon}
            loading
            disabled
          />
        {:else if configOptions.length > 0}
          {#each configOptions as option (option.key)}
            <ConfigOptionPicker
              {option}
              value={selectedConfigValues[option.key]}
              onchange={(value) => changeConfigOption(option.key, value)}
              showRefresh={isModelOption(option)}
              onrefresh={() => void loadAgentCapabilities(true)}
            />
          {/each}
        {:else if capabilitiesError !== null}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_model_default()}
            icon={modelIcon}
            contentClass="w-72 p-0"
          >
            <div class="flex flex-col gap-2 p-2 text-ui">
              <p class="text-muted-foreground">{m.new_session_models_unavailable()}</p>
              <p class="text-destructive break-words text-2xs">{capabilitiesError}</p>
              <Button
                variant="outline"
                size="sm"
                class="self-start"
                onclick={() => void loadAgentCapabilities(true)}
              >
                {m.common_retry()}
              </Button>
            </div>
          </PickerPill>
        {:else if capabilities !== null && capabilitiesKey === probeKey}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_model_default()}
            icon={modelIcon}
            contentClass="w-72 p-2"
          >
            <p class="text-muted-foreground text-ui">{m.new_session_models_empty()}</p>
          </PickerPill>
        {:else}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_model_default()}
            icon={modelIcon}
            disabled
          />
        {/if}

        <span class="flex-1"></span>

        <Button
          variant="ghost"
          size="icon-sm"
          class="text-muted-foreground hover:text-foreground rounded-full"
          title={m.new_session_settings_title()}
          aria-label={m.new_session_settings_title()}
          onclick={() => (showSettings = true)}
        >
          <Settings2Icon class="size-4" />
        </Button>

        <ComposerSendButton
          {busy}
          disabled={!canSubmit || busy}
          title={startTitle}
          label={m.new_session_start()}
          onclick={() => void handleStartSession()}
        />
      {/snippet}
    </ComposerCard>

    <div class="min-h-4 text-center text-ui">
      {#if busy}
        <span class="text-muted-foreground">{m.new_session_starting()}</span>
      {:else if blockingHint !== ""}
        <span class="text-destructive">{blockingHint}</span>
      {:else if capabilitiesError !== null && capabilitiesKey === probeKey}
        <span class="text-muted-foreground">{m.new_session_models_unavailable()}</span>
      {:else}
        <span class="text-muted-foreground hidden sm:inline">{m.new_session_enter_hint()}</span>
        <span class="text-muted-foreground sm:hidden">{m.new_session_tap_hint()}</span>
      {/if}
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

<SessionSettingsDialog
  bind:open={showSettings}
  bind:pathMode
  bind:branch
  bind:baseBranch
  bind:sessionMode
  bind:extraArgsText
  bind:manualAgent
  bind:agentId
  {isProvisionerMode}
  forkActive={forkSessionId !== ""}
  {provisionerName}
  {testBusy}
  onTest={() => void runProvisionerTest(provisionerName)}
  onModeChange={onSessionModeChange}
  onAgentIdInput={onManualAgentInput}
/>

<Dialog.Root bind:open={testDialogOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{m.new_session_provisioner_test_title({ name: provisionerName })}</Dialog.Title>
      <Dialog.Description>
        {m.new_session_provisioner_test_description()}
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3 py-2 text-ui">
      {#if testBusy}
        <div class="text-muted-foreground flex flex-col items-center justify-center gap-2 py-8">
          <LoaderCircleIcon class="text-primary size-6 animate-spin" />
          <span>{m.new_session_provisioner_starting()}</span>
        </div>
      {:else if testResult}
        <div class="flex items-center gap-2 font-medium">
          {#if testResult.ok}
            <CheckCircle2Icon class="text-ok size-5" />
            <span class="text-ok">{m.new_session_test_success()}</span>
            {#if testResult.node_id}
              <span class="text-muted-foreground font-mono text-2xs">({testResult.node_id})</span>
            {/if}
          {:else}
            <XCircleIcon class="text-destructive size-5" />
            <span class="text-destructive">{m.new_session_test_failed()}</span>
          {/if}
        </div>

        {#if testResult.error}
          <div
            class="bg-destructive/10 text-destructive border-destructive/20 rounded-md border p-2.5 font-mono text-2xs whitespace-pre-wrap"
          >
            {testResult.error}
          </div>
        {/if}

        {#if testResult.log_lines && testResult.log_lines.length > 0}
          <div class="flex flex-col gap-1">
            <span class="text-muted-foreground text-2xs">{m.new_session_run_log()}</span>
            <div
              class="bg-muted text-foreground max-h-48 overflow-y-auto rounded-md p-2 font-mono text-2xs leading-tight whitespace-pre-wrap"
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
