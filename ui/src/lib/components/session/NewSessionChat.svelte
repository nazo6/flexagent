<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import AgentIcon from "#lib/components/agent/AgentIcon.svelte";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import ComposerCard from "#lib/components/composer/ComposerCard.svelte";
  import ComposerMoreButton from "#lib/components/composer/ComposerMoreButton.svelte";
  import ComposerSendButton from "#lib/components/composer/ComposerSendButton.svelte";
  import ComposerTextarea from "#lib/components/composer/ComposerTextarea.svelte";
  import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.svelte";
  import ModePicker from "#lib/components/composer/ModePicker.svelte";
  import PickerItem from "#lib/components/composer/PickerItem.svelte";
  import PickerPill from "#lib/components/composer/PickerPill.svelte";
  import { createComposerAttachments } from "#lib/composer-attachments.svelte.ts";
  import { agentHint, agentsAvailableOn } from "#lib/agents.ts";
  import type {
    AgentCapabilitiesPreview,
    AgentSummary,
    ConfigOptionValue,
    InitialConfig,
    ModeInfo,
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
    shouldSubmitOnEnter,
    type SessionModePreference,
  } from "#lib/session-prefs.ts";
  import { composerDrafts, NEW_SESSION_DRAFT_ID } from "#lib/stores/composer-drafts.ts";
  import { prefs } from "#lib/stores/prefs.svelte.ts";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import { nodeSettingsPath, resolveDefaultNodeId } from "#lib/settings-routes.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import { toast } from "svelte-sonner";
  import { onMount, untrack } from "svelte";
  import BoxIcon from "@lucide/svelte/icons/box";
  import CheckCircle2Icon from "@lucide/svelte/icons/check-circle-2";
  import FileIcon from "@lucide/svelte/icons/file";
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import GitBranchIcon from "@lucide/svelte/icons/git-branch";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import PaperclipIcon from "@lucide/svelte/icons/paperclip";
  import SparklesIcon from "@lucide/svelte/icons/sparkles";
  import ServerIcon from "@lucide/svelte/icons/server";
  import XCircleIcon from "@lucide/svelte/icons/x-circle";
  import XIcon from "@lucide/svelte/icons/x";

  type PathMode = "existing" | "new_worktree";
  type RunTarget = "node" | "provisioner";

  /**
   * Deep-link parameters, read once when the screen is created.
   *
   * The URL is an entry point, not a reactive input: applying it again while
   * the user edits the form would overwrite their selections.
   */
  const initialUrl = untrack(() => {
    const params = page.url.searchParams;
    const forkUptoRaw = params.get("fork_upto");
    const forkUpto = forkUptoRaw === null ? Number.NaN : Number(forkUptoRaw);
    return {
      project: params.get("project") ?? "",
      node: params.get("node") ?? "",
      path: params.get("path") ?? "",
      forkSession: params.get("fork_session") ?? "",
      forkUpTo: Number.isSafeInteger(forkUpto) && forkUpto >= 0 ? forkUpto : null,
      agent: params.get("agent") ?? "",
    };
  });

  /** Project explicitly chosen by the user (empty = deep link or first project). */
  let chosenProjectId = $state("");
  /** Node explicitly chosen by the user (`null` = follow the default resolution). */
  let chosenNodeId = $state<string | null>(null);
  /** Whether the deep-linked node still applies (cleared once the user changes project). */
  let deepLinkNodeActive = $state(initialUrl.node !== "");
  /** Working directory explicitly chosen by the user (empty = default candidate). */
  let chosenPath = $state(initialUrl.path);
  /** Agent explicitly chosen by the user (empty = remembered / first available). */
  let chosenAgentId = $state(initialUrl.agent);
  /**
   * Initial mode explicitly chosen by the user (`null` = remembered mode).
   * A deep-linked agent starts from its own default because mode IDs are
   * agent-specific.
   */
  let chosenMode = $state<SessionModePreference | null>(initialUrl.agent === "" ? null : "default");
  /**
   * Manual agent-ID input (`null` = follow the catalog).
   *
   * A deep link to an agent missing from the catalog starts in manual mode; the
   * user's toggle (or a catalog selection) fixes the choice afterwards.
   */
  let manualAgentChoice = $state<boolean | null>(initialUrl.agent === "" ? false : null);

  let runTarget = $state<RunTarget>("node");
  let provisionerName = $state("");
  const provisioners = $state<ProvisionerSummary[]>([]);
  /** ACP Registry catalog + per-node availability. */
  let catalog = $state<AgentSummary[]>([]);
  let catalogLoading = $state(true);
  let catalogError = $state<string | null>(null);
  /** Node whose catalog is currently loaded (never mix two nodes' lists). */
  let catalogNodeId = $state("");
  /** Guards against stale responses when the node changes / the user retries. */
  let catalogGeneration = 0;
  let provisionerLoadError = $state<string | null>(null);
  let pathMode = $state<PathMode>("existing");
  let branch = $state("");
  let baseBranch = $state("");
  const draft = composerDrafts.get(NEW_SESSION_DRAFT_ID);
  let promptText = $state(draft.text);
  let busy = $state(false);
  let browserOpen = $state(false);
  let fileBrowserOpen = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  let useCustomPath = $state(initialUrl.path !== "");

  const composer = createComposerAttachments(draft.attachments);
  const attachments = $derived(composer.attachments);

  // Keep the prompt while the user visits another screen and comes back.
  $effect(() => {
    composerDrafts.setText(NEW_SESSION_DRAFT_ID, promptText);
  });

  $effect(() => {
    composerDrafts.setAttachments(NEW_SESSION_DRAFT_ID, attachments);
  });

  // Picker popovers
  let projectOpen = $state(false);
  let directoryOpen = $state(false);
  let environmentOpen = $state(false);
  let agentOpen = $state(false);
  let moreOpen = $state(false);

  // Fork inheritance
  let forkSessionId = $state(initialUrl.forkSession);
  let forkUpToNodeSeq = $state<number | null>(initialUrl.forkUpTo);

  // Launch options
  let extraArgsText = $state("");

  // Agent capability probe (models / modes)
  let capabilities = $state<AgentCapabilitiesPreview | null>(null);
  let capabilitiesKey = $state("");
  let capabilitiesLoading = $state(false);
  let capabilitiesError = $state<string | null>(null);
  let capabilitiesGeneration = 0;
  /** Per-screen probe cache so re-selecting the same agent is instant. */
  const capabilitiesCache = new Map<string, AgentCapabilitiesPreview>();
  let selectedConfigValues = $state<Record<string, ConfigOptionValue>>({});

  // Provisioner connectivity test
  let testBusy = $state(false);
  let testResult = $state<ProvisionerTestResponse | null>(null);
  let testDialogOpen = $state(false);

  const projects = $derived(sync.projects);
  const selectedProject = $derived(
    projects.find((project) => project.project_id === chosenProjectId) ??
      projects.find((project) => project.project_id === initialUrl.project) ??
      projects[0] ??
      null,
  );

  const isProvisionerMode = $derived(runTarget === "provisioner");
  const nodes = $derived(sync.nodes);
  const onlineNodes = $derived(nodes.filter((node) => node.is_online));
  /** Deep-link node, only once it exists in the current node list. */
  const deepLinkNodeId = $derived(
    deepLinkNodeActive &&
      initialUrl.node !== "" &&
      nodes.some((node) => node.node_id === initialUrl.node)
      ? initialUrl.node
      : "",
  );
  const defaultNodeId = $derived.by(() => {
    if (nodes.length === 0) return "";
    if (deepLinkNodeId !== "") return deepLinkNodeId;
    // Read the project and node list reactively (so a project / node change
    // re-resolves the default), but not session history: session activity
    // reorders candidates and must not move the selection.
    const project = selectedProject;
    return pickDefaultNode(
      nodes,
      project,
      untrack(() => sync.sessions),
    );
  });
  const nodeId = $derived(
    chosenNodeId !== null && nodes.some((node) => node.node_id === chosenNodeId)
      ? chosenNodeId
      : defaultNodeId,
  );
  const selectedNode = $derived(nodes.find((n) => n.node_id === nodeId) ?? null);
  const nodeAvailability = $derived(getNodeAvailability(selectedNode, sync.wsConnected));
  /** Whether `catalog` belongs to the current node (never mix two nodes' lists). */
  const catalogReady = $derived(nodeId !== "" && catalogNodeId === nodeId);
  // Agents that can actually be launched on this node (based on the catalog's
  // installed_nodes; npx/uvx distributed agents are included only once enabled
  // via "add from catalog")
  const availableAgents = $derived(catalogReady ? agentsAvailableOn(catalog, nodeId) : []);
  const availableAgentIds = $derived(availableAgents.map((agent) => agent.id));
  /** Remembered agent for the project, as long as this node still offers it. */
  const rememberedAgent = $derived.by(() => {
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "" || !catalogReady) return "";
    const saved = loadNewSessionPrefs(projectId);
    return saved !== null && availableAgentIds.includes(saved.agent) ? saved.agent : "";
  });
  const effectiveAgent = $derived(
    chosenAgentId.trim() !== ""
      ? chosenAgentId.trim()
      : rememberedAgent !== ""
        ? rememberedAgent
        : (availableAgentIds[0] ?? ""),
  );
  const selectedAgent = $derived(
    availableAgents.find((agent) => agent.id === effectiveAgent) ?? null,
  );
  /**
   * Whether to show the free-form agent ID input instead of the catalog list.
   * Until the user (or a selection) fixes the choice, a deep-linked agent that
   * the loaded catalog does not contain starts in manual mode.
   */
  const manualAgent = $derived(
    manualAgentChoice ?? (catalogReady && !availableAgentIds.includes(initialUrl.agent)),
  );

  const agentsSettingsHref = $derived(
    nodeSettingsPath(
      "agents",
      (!isProvisionerMode && nodeId !== "" ? nodeId : null) ??
        resolveDefaultNodeId(
          sync.connection.role === "hub",
          nodes,
          sync.connection.systemInfo?.node_id,
        ),
    ),
  );
  const selectedProvisioner = $derived(
    provisioners.find((p) => p.name === provisionerName) ?? provisioners[0] ?? null,
  );

  /** Loads ephemeral VM provisioners (hub only; retryable from the UI). */
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
   * The hub rejects the unscoped `GET /api/v1/agents` (every catalog is
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
      catalogNodeId = target;
      catalogError = null;
    } catch (error) {
      if (generation !== catalogGeneration) return;
      catalog = [];
      catalogNodeId = target;
      catalogError = error instanceof Error ? error.message : String(error);
      problems.report("load agents", error);
    } finally {
      if (generation === catalogGeneration) catalogLoading = false;
    }
  }

  // Ephemeral VM provisioner definitions (only available when connected to the hub)
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

  // Resolve working directory candidates (bindings + recent sessions) in last-used order
  const pathCandidates = $derived.by(() =>
    buildPathCandidates(selectedProject, nodeId, sync.sessions),
  );
  /**
   * Pin the automatic path per project × node (and its empty → ready
   * transition). Session updates reorder candidates but must not replace the
   * directory the user is about to use.
   */
  const autoPathKey = $derived(
    `${selectedProject?.project_id ?? ""}\u0000${nodeId}\u0000${pathCandidates.length === 0 ? "empty" : "ready"}`,
  );
  const defaultPath = $derived.by(() => {
    void autoPathKey;
    return untrack(() => pickDefaultCandidate(pathCandidates)?.path ?? "");
  });
  /** Selected working directory (explicit choice, or the pinned default). */
  const localPath = $derived.by(() => {
    // A typed / browsed path is used as-is; a picked candidate only while it
    // still exists in the list (otherwise fall back to the default).
    if (chosenPath !== "" && (useCustomPath || pathCandidates.some((c) => c.path === chosenPath))) {
      return chosenPath;
    }
    if (defaultPath !== "" && pathCandidates.some((candidate) => candidate.path === defaultPath)) {
      return defaultPath;
    }
    return pickDefaultCandidate(pathCandidates)?.path ?? "";
  });
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
   * Some agents publish dependent options such as
   * `effort` only after a model is selected. Before the first probe the only
   * model key we know is the conventional `model` remembered for this project
   * and agent. It is scoped by agent because model values are agent-specific
   * (OpenCode expects `provider/model`, Gemini CLI uses bare IDs).
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
    if (projectId === "" || effectiveAgent === "") return [];
    const saved = loadSessionConfig(projectId, effectiveAgent)["model"];
    return saved === undefined ? [] : [{ key: "model", value: saved }];
  }

  /** UI cache key for one probe request (agent / path / initial config). */
  function probeCacheKey(key: string, config: InitialConfig[]): string {
    return `${key}\u0000${JSON.stringify(config)}`;
  }

  /** Stable key for the probe target (node / agent / working directory). */
  const probeKey = $derived(
    isProvisionerMode ||
      !catalogReady ||
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

  /**
   * Modes reported by the probed agent (`null` while it has not answered yet).
   *
   * The initial-mode choices come from the agent instead of a fixed list, so
   * agent-specific IDs (OpenCode's `build`, for example) are passed through
   * unchanged.
   */
  const availableModes = $derived<ModeInfo[] | null>(
    capabilitiesKey === probeKey && capabilities !== null
      ? (capabilities.available_modes ?? [])
      : null,
  );

  /**
   * Remembered initial mode for the project (agent-specific IDs are not mixed).
   */
  const rememberedMode = $derived.by(() => {
    const projectId = selectedProject?.project_id ?? "";
    if (projectId === "" || !catalogReady) return "default" as SessionModePreference;
    return loadNewSessionPrefs(projectId)?.mode ?? "default";
  });

  /**
   * Initial mode sent with the session. A remembered mode the probed agent does
   * not offer is ignored instead of failing `session/set_mode`.
   */
  const sessionMode = $derived.by<SessionModePreference>(() => {
    const proposed = chosenMode ?? rememberedMode;
    if (proposed === "default" || availableModes === null) return proposed;
    return availableModes.some((mode) => mode.mode_id === proposed) ? proposed : "default";
  });

  function sameConfig(left: InitialConfig[], right: InitialConfig[]): boolean {
    return JSON.stringify(left) === JSON.stringify(right);
  }

  /** Whether two selected config maps hold the same keys and values. */
  function sameConfigMap(
    left: Record<string, ConfigOptionValue>,
    right: Record<string, ConfigOptionValue>,
  ): boolean {
    const keys = Object.keys(left);
    if (keys.length !== Object.keys(right).length) return false;
    return keys.every(
      (key) => key in right && JSON.stringify(left[key]) === JSON.stringify(right[key]),
    );
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
    const saved =
      projectId === "" || effectiveAgent === "" ? {} : loadSessionConfig(projectId, effectiveAgent);
    const resolved = resolveInitialConfig(preview.config_options ?? [], saved);
    // Replacing the object invalidates every reader even when the selection did
    // not change. The probe effect applies cached results synchronously, so a
    // needless replacement there would make the effect re-run forever.
    if (!sameConfigMap(resolved, selectedConfigValues)) {
      selectedConfigValues = resolved;
    }

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
      capabilitiesGeneration += 1;
      capabilities = null;
      capabilitiesKey = "";
      capabilitiesLoading = false;
      capabilitiesError = null;
      selectedConfigValues = {};
      return;
    }
    // Drop the previous agent's/path's options immediately; the new probe
    // starts from a clean loading state.
    capabilities = null;
    capabilitiesKey = "";
    capabilitiesLoading = true;
    capabilitiesError = null;
    selectedConfigValues = {};
    // Apply a cached probe inside `untrack`: the cache-hit path runs
    // synchronously and writes `capabilities` / `selectedConfigValues`. If this
    // effect subscribed to those writes it would re-run, take the same cache hit
    // again and loop until Svelte throws `effect_update_depth_exceeded`.
    untrack(() => {
      void loadAgentCapabilities(false, currentModelConfig());
    });
  });

  function changeConfigOption(key: string, value: ConfigOptionValue): void {
    selectedConfigValues = { ...selectedConfigValues, [key]: value };
    const projectId = selectedProject?.project_id ?? "";
    if (projectId !== "" && effectiveAgent !== "") {
      saveSessionConfigOption(projectId, effectiveAgent, key, value);
    }

    // Selecting a model can reveal dependent options (for example effort
    // levels) that the previous probe did not return.
    const option = (capabilities?.config_options ?? []).find((candidate) => candidate.key === key);
    if (option !== undefined && isModelOption(option)) {
      void loadAgentCapabilities(true, [{ key, value }]);
    }
  }

  function initialConfig(): { key: string; value: ConfigOptionValue }[] {
    // Only send options that the current agent reported and that were validated
    // against its choices. Without a completed probe we cannot tell whether a
    // remembered value still exists for this agent (or even belongs to it), and
    // the agent rejects the whole `set_config_option` request when a model is
    // unknown. A missing value is better than a failed application: the agent
    // then keeps its own default.
    if (probeKey === "" || capabilities === null || capabilitiesKey !== probeKey) return [];
    return Object.entries(selectedConfigValues).map(([key, value]) => ({ key, value }));
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
      if (onlineNodes.length === 0) return m.new_session_no_online_nodes();
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
    environmentOpen = false;
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
        initial_prompt:
          promptToSend === "" && attachments.length === 0
            ? null
            : { text: promptToSend, attachments: [...attachments] },
        initial_config: initialConfig(),
        mode: isProvisionerMode || sessionMode === "default" ? null : sessionMode,
        extra_args: extraArgs,
        fork: forkSessionId
          ? {
              from_session_id: forkSessionId,
              up_to_node_seq: forkUpToNodeSeq,
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
      promptText = "";
      composer.clear();
      composerDrafts.clear(NEW_SESSION_DRAFT_ID);
      await goto(`/sessions/${response.session_id}`);
    } catch (error) {
      // The form state (including the prompt) is intentionally kept so the
      // user can retry without retyping.
      problems.report("create session", error);
      busy = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (shouldSubmitOnEnter(event, prefs.sendShortcut)) {
      event.preventDefault();
      void handleStartSession();
    }
  }

  /** Save the launch settings for this project (best effort, local device only). */
  function persistPrefs() {
    const projectId = selectedProject?.project_id ?? "";
    // Do not remember ephemeral VMs because agent availability differs per node
    if (projectId === "" || isProvisionerMode) return;
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
    chosenProjectId = projectId;
    // Everything else is project-scoped: re-resolve node, path and memory.
    chosenNodeId = null;
    deepLinkNodeActive = false;
    chosenPath = "";
    chosenAgentId = "";
    chosenMode = null;
    manualAgentChoice = false;
    useCustomPath = false;
    projectOpen = false;
  }

  function selectNode(id: string): void {
    runTarget = "node";
    chosenNodeId = id;
    chosenPath = "";
    useCustomPath = false;
    environmentOpen = false;
  }

  function selectProvisioner(name: string): void {
    runTarget = "provisioner";
    provisionerName = name;
    // Keep the popover open: it also hosts the clone branch and connectivity test.
  }

  function selectAgent(id: string): void {
    // Mode IDs are agent-specific: a selection for the previous agent no longer applies.
    if (id !== effectiveAgent) chosenMode = "default";
    chosenAgentId = id;
    manualAgentChoice = false;
    agentOpen = false;
    persistPrefs();
  }

  function onManualAgentInput(): void {
    manualAgentChoice = true;
    // The ID being typed is a different agent until proven otherwise.
    chosenMode = "default";
    persistPrefs();
  }

  /** Switch between the catalog list and the free-form agent ID input. */
  function toggleManualAgent(): void {
    manualAgentChoice = !manualAgent;
    // Start from the agent currently in effect instead of an empty field.
    if (manualAgentChoice && chosenAgentId === "") chosenAgentId = effectiveAgent;
  }

  function selectPath(path: string): void {
    chosenPath = path;
    useCustomPath = false;
    directoryOpen = false;
  }

  function resetPathToDefault(): void {
    chosenPath = "";
    useCustomPath = false;
  }

  function openFolderBrowser(): void {
    directoryOpen = false;
    browserOpen = true;
  }

  function openFileBrowser(): void {
    fileBrowserOpen = true;
  }

  function onFileInputChange(event: Event): void {
    const files = (event.currentTarget as HTMLInputElement).files;
    if (files) {
      void composer.addImageFiles(files);
      (event.currentTarget as HTMLInputElement).value = "";
    }
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
  <AgentIcon icon={selectedAgent?.icon ?? null} class="size-3.5" />
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
      <p class="text-muted-foreground max-w-md text-[13px]">
        {m.new_session_description()}
      </p>
    </div>

    {#if forkSessionId}
      <div
        class="bg-signal-soft/70 border-signal/25 flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]"
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
          class="h-6 shrink-0 px-2 text-[13px]"
          onclick={() => {
            forkSessionId = "";
            forkUpToNodeSeq = null;
          }}
        >
          <XIcon class="mr-1 size-3.5" />
          {m.new_session_fork_clear()}
        </Button>
      </div>
    {/if}

    {#if sync.projectsLoading && projects.length === 0}
      <div
        class="bg-chip text-muted-foreground flex items-center justify-center gap-2 rounded-lg border border-border p-3 text-[13px]"
      >
        <LoaderCircleIcon class="size-3.5 animate-spin" />
        {m.common_loading()}
      </div>
    {:else if projects.length === 0 && sync.projectsError === null}
      <div
        class="border-warn/35 bg-warn-soft/60 flex flex-wrap items-center justify-between gap-2 rounded-lg border p-3 text-[13px]"
      >
        <span class="flex items-center gap-2">
          <FolderGit2Icon class="size-4 shrink-0" />
          {m.new_session_no_projects()}
        </span>
        <a
          href="/projects"
          class="bg-background hover:bg-muted rounded-md border px-2.5 py-1 text-[13px] font-medium"
        >
          {m.new_session_register_project()}
        </a>
      </div>
    {:else if sync.projectsError !== null}
      <div
        class="border-danger/30 bg-danger-soft/60 text-danger flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]"
      >
        <span class="truncate">{sync.projectsError}</span>
        <Button variant="ghost" size="xs" onclick={() => void sync.refreshProjects()}>
          {m.common_retry()}
        </Button>
      </div>
    {/if}

    <ComposerCard>
      {#snippet above()}
        <div class="flex flex-wrap items-center gap-1.5 px-1">
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
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
                >
                  {m.new_session_manage_projects()}
                </a>
              </div>
            {:else}
              <div class="p-1">
                <a
                  href="/projects"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
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
              <p class="text-muted-foreground p-2 text-[13px]">{m.new_session_ephemeral_note()}</p>
            {:else if pathMode === "new_worktree"}
              <div class="flex flex-col gap-2 p-2">
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[11px]"
                    >{m.new_session_new_worktree()}</span
                  >
                  <Input bind:value={branch} placeholder="feat/new-task" class="h-8 text-[13px]" />
                </div>
                <div class="flex flex-col gap-1">
                  <span class="text-muted-foreground text-[11px]"
                    >{m.new_session_base_branch()}</span
                  >
                  <Input bind:value={baseBranch} placeholder="main" class="h-8 text-[13px]" />
                </div>
                <p class="text-muted-foreground text-[11px]">
                  {m.new_session_worktree_description()}
                </p>
                <button
                  type="button"
                  class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
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
                  bind:value={chosenPath}
                  placeholder={m.new_session_path_placeholder()}
                  class="h-8 font-mono text-[13px]"
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
                  <p class="text-muted-foreground text-[11px]">
                    {m.new_session_unregistered_path()}
                  </p>
                {/if}
                {#if selectedCandidate && !selectedCandidate.exists}
                  <p class="text-destructive text-[11px]">{m.new_session_path_not_exists()}</p>
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
                      class="text-muted-foreground block truncate text-[11px] {candidate.exists
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
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
                  onclick={openFolderBrowser}
                >
                  {m.new_session_browse()}
                </button>
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
                  onclick={() => {
                    if (chosenPath === "") chosenPath = localPath;
                    useCustomPath = true;
                  }}
                >
                  {m.new_session_enter_path()}
                </button>
                <button
                  type="button"
                  class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
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
                <p class="text-muted-foreground text-[13px]">
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
                    class="text-muted-foreground hover:text-foreground text-[11px] underline underline-offset-2"
                  >
                    {m.new_session_go_to_project_settings()}
                  </a>
                </div>
                {#each otherNodesWithCandidates as otherNode (otherNode.node_id)}
                  <button
                    type="button"
                    class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
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
                class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase"
              >
                {m.new_session_node_section()}
              </div>
              {#if onlineNodes.length === 0}
                <p class="text-muted-foreground px-2 py-1.5 text-[13px]">
                  {nodes.length === 0 ? m.new_session_no_nodes() : m.new_session_no_online_nodes()}
                </p>
              {:else}
                {#each nodes as node (node.node_id)}
                  {@const availability = getNodeAvailability(node, sync.wsConnected)}
                  <PickerItem
                    selected={!isProvisionerMode && node.node_id === nodeId}
                    disabled={!availability.isAvailable}
                    onselect={() => selectNode(node.node_id)}
                  >
                    <span class="flex items-center gap-1.5">
                      <span class="size-1.5 shrink-0 rounded-full {availability.dotColorClass}"
                      ></span>
                      <span class="truncate">{node.name}</span>
                    </span>
                    <span class="text-muted-foreground block truncate text-[11px]">
                      {availability.statusText}
                    </span>
                  </PickerItem>
                {/each}
              {/if}

              <div class="border-t p-1 pt-2">
                <div
                  class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase"
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
                      <span class="text-muted-foreground block truncate text-[11px]">
                        {provisioner.name}
                      </span>
                    </PickerItem>
                  {/each}
                {:else if provisionerLoadError !== null}
                  <div class="flex items-center gap-1.5 px-2 py-1 text-[11px]">
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
                  <p class="text-muted-foreground px-2 py-1 text-[11px]">
                    {m.new_session_provisioner_undefined()}
                  </p>
                  <p class="text-muted-foreground px-2 pb-1 text-[11px]">
                    {m.new_session_provisioner_config_hint()}
                  </p>
                {/if}

                {#if isProvisionerMode && selectedProvisioner !== null}
                  <div class="mt-2 flex flex-col gap-2 border-t px-2 pt-2">
                    <div class="flex flex-col gap-1">
                      <span class="text-muted-foreground text-[11px]">
                        {m.new_session_clone_branch()}
                      </span>
                      <Input
                        bind:value={branch}
                        placeholder={m.new_session_clone_branch_placeholder()}
                        class="h-8 text-[13px]"
                      />
                    </div>
                    <div class="flex items-center justify-between gap-2">
                      <span class="text-muted-foreground min-w-0 truncate text-[11px]">
                        {selectedProvisioner.name}
                      </span>
                      <Button
                        variant="outline"
                        size="sm"
                        class="h-7 shrink-0 px-2 text-[13px]"
                        disabled={testBusy}
                        title={m.new_session_provisioner_test_button_title()}
                        onclick={() => void runProvisionerTest(provisionerName)}
                      >
                        {m.new_session_test()}
                      </Button>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          </PickerPill>

          {#if attachments.length > 0}
            <div class="flex w-full flex-wrap gap-2 pt-1">
              {#each attachments as attachment, index (index)}
                <div
                  class="bg-chip border-border flex items-center gap-1.5 rounded-md border px-2 py-1 text-[13px]"
                >
                  {#if attachment.data_b64}
                    <img
                      src={`data:${attachment.mime_type};base64,${attachment.data_b64}`}
                      alt={attachment.file_name}
                      class="size-8 rounded object-cover"
                    />
                  {:else}
                    <FileIcon class="text-muted-foreground size-3.5" />
                  {/if}
                  <span class="max-w-40 truncate">{attachment.file_name}</span>
                  <button
                    type="button"
                    class="text-muted-foreground hover:text-foreground"
                    title={m.chat_attachment_remove()}
                    onclick={() => composer.remove(index)}
                  >
                    <XIcon class="size-3.5" />
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/snippet}

      <ComposerTextarea
        bind:value={promptText}
        onkeydown={onKeydown}
        onpaste={composer.onPaste}
        ondrop={composer.onDrop}
        rows={3}
        placeholder={m.new_session_prompt_placeholder()}
        ariaLabel={m.new_session_prompt_label()}
        disabled={busy}
      />

      {#snippet selectors()}
        <PickerPill
          label={m.new_session_agent()}
          value={agentLabel}
          icon={agentIcon}
          class="shrink-0"
          contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
          bind:open={agentOpen}
          disabled={forkSessionId !== ""}
        >
          {#if manualAgent}
            <div class="flex flex-col gap-2 p-2">
              <Input
                bind:value={chosenAgentId}
                oninput={onManualAgentInput}
                placeholder={m.new_session_agent_id_placeholder()}
                class="h-8 font-mono text-[13px]"
                disabled={forkSessionId !== ""}
              />
              <p class="text-muted-foreground text-[11px]">{m.new_session_agent_choose_hint()}</p>
            </div>
          {:else if catalogLoading}
            <p class="text-muted-foreground flex items-center gap-1.5 px-2 py-2 text-[13px]">
              <LoaderCircleIcon class="size-3.5 animate-spin" />
              {m.common_loading()}
            </p>
          {:else if catalogError !== null}
            <div class="flex flex-col gap-2 p-2">
              <p class="text-destructive break-words text-[11px]">{catalogError}</p>
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
            <p class="text-muted-foreground p-2 text-[13px]">{m.new_session_no_agents()}</p>
          {:else}
            <div class="max-h-72 overflow-y-auto p-1">
              {#each availableAgents as agent (agent.id)}
                <PickerItem
                  selected={agent.id === effectiveAgent}
                  onselect={() => selectAgent(agent.id)}
                >
                  <span class="flex min-w-0 items-center gap-2">
                    <AgentIcon icon={agent.icon} class="size-4" />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate">{agent.name}</span>
                      <span class="text-muted-foreground block truncate font-mono text-[11px]">
                        {agent.id} — {agentHint(agent)}
                      </span>
                    </span>
                  </span>
                </PickerItem>
              {/each}
            </div>
          {/if}
          <div class="border-t p-1">
            <button
              type="button"
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block w-full rounded-md px-2 py-1.5 text-left text-[13px] disabled:pointer-events-none disabled:opacity-50"
              disabled={forkSessionId !== ""}
              onclick={toggleManualAgent}
            >
              {manualAgent ? m.new_session_choose_from_list() : m.new_session_enter_id()}
            </button>
            <a
              href={agentsSettingsHref}
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
            >
              {m.new_session_manage_agents()}
            </a>
          </div>
        </PickerPill>

        {#if !isProvisionerMode}
          {#if probeKey !== "" && (capabilitiesLoading || capabilitiesKey !== probeKey)}
            <ModePicker modes={[]} loading class="shrink-0" onchange={() => {}} />
          {:else if availableModes !== null && availableModes.length > 0}
            <ModePicker
              modes={availableModes}
              value={sessionMode === "default" ? null : sessionMode}
              allowDefault
              class="shrink-0"
              onchange={(modeId) => {
                chosenMode = modeId ?? "default";
                persistPrefs();
              }}
            />
          {/if}
        {/if}

        {#if isProvisionerMode}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_models_after_start()}
            icon={modelIcon}
            class="shrink-0"
            disabled
          />
        {:else if probeKey !== "" && (capabilitiesLoading || capabilitiesKey !== probeKey)}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_models_loading()}
            icon={modelIcon}
            class="shrink-0"
            loading
            disabled
          />
        {:else if configOptions.length > 0}
          {#each configOptions as option (option.key)}
            <ConfigOptionPicker
              {option}
              value={selectedConfigValues[option.key]}
              class="shrink-0"
              favoriteScope={effectiveAgent}
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
            class="shrink-0"
            contentClass="w-72 p-0"
          >
            <div class="flex flex-col gap-2 p-2 text-[13px]">
              <p class="text-muted-foreground">{m.new_session_models_unavailable()}</p>
              <p class="text-destructive break-words text-[11px]">{capabilitiesError}</p>
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
            class="shrink-0"
            contentClass="w-72 p-2"
          >
            <p class="text-muted-foreground text-[13px]">{m.new_session_models_empty()}</p>
          </PickerPill>
        {:else}
          <PickerPill
            label={m.new_session_model()}
            value={m.new_session_model_default()}
            icon={modelIcon}
            class="shrink-0"
            disabled
          />
        {/if}
      {/snippet}

      {#snippet actions()}
        <input
          bind:this={fileInput}
          type="file"
          accept="image/*"
          multiple
          class="hidden"
          aria-label={m.chat_attach_image()}
          onchange={onFileInputChange}
        />
        <Button
          variant="ghost"
          size="icon-sm"
          class="rounded-full"
          title={m.chat_attach_image()}
          aria-label={m.chat_attach_image()}
          onclick={() => fileInput?.click()}
        >
          <PaperclipIcon class="size-4" />
        </Button>

        <ComposerMoreButton
          label={m.composer_more_options()}
          active={extraArgsText.trim() !== ""}
          bind:open={moreOpen}
          contentClass="w-[min(20rem,calc(100vw-2rem))]"
        >
          <div class="flex flex-col gap-2 p-2">
            {#if !isProvisionerMode && nodeId !== ""}
              <button
                type="button"
                class="hover:bg-accent flex items-center gap-2 rounded-md px-1.5 py-1.5 text-left text-[13px]"
                onclick={() => {
                  moreOpen = false;
                  openFileBrowser();
                }}
              >
                <FileIcon class="text-muted-foreground size-4" />
                {m.chat_attach_file()}
              </button>
              <div class="border-t"></div>
            {/if}
            <div class="flex flex-col gap-1">
              <Label for="new-session-extra-args" class="text-muted-foreground text-[11px]">
                {m.new_session_extra_args()}
              </Label>
              <Input
                id="new-session-extra-args"
                bind:value={extraArgsText}
                placeholder={m.new_session_extra_args_placeholder()}
                class="h-8 font-mono text-[13px]"
              />
            </div>
          </div>
        </ComposerMoreButton>

        <ComposerSendButton
          {busy}
          disabled={!canSubmit || busy}
          title={startTitle}
          label={m.new_session_start()}
          onclick={() => void handleStartSession()}
        />
      {/snippet}
    </ComposerCard>

    <div class="min-h-4 text-center text-[13px]">
      {#if busy}
        <span class="text-muted-foreground">{m.new_session_starting()}</span>
      {:else if blockingHint !== ""}
        <span class="text-destructive">{blockingHint}</span>
      {:else if capabilitiesError !== null && capabilitiesKey === probeKey}
        <span class="text-muted-foreground">{m.new_session_models_unavailable()}</span>
      {:else if canSubmit}
        <span class="text-muted-foreground hidden sm:inline">
          {prefs.sendShortcut === "enter"
            ? m.new_session_enter_hint()
            : m.new_session_enter_hint_ctrl()}
        </span>
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
    chosenPath = path;
    useCustomPath = true;
  }}
/>

<FolderBrowserDialog
  bind:open={fileBrowserOpen}
  {nodeId}
  files={true}
  title={m.chat_attach_file()}
  onSelect={composer.addLocalFile}
/>

<Dialog.Root bind:open={testDialogOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{m.new_session_provisioner_test_title({ name: provisionerName })}</Dialog.Title>
      <Dialog.Description>
        {m.new_session_provisioner_test_description()}
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3 py-2 text-[13px]">
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
              <span class="text-muted-foreground font-mono text-[11px]">({testResult.node_id})</span
              >
            {/if}
          {:else}
            <XCircleIcon class="text-destructive size-5" />
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
              class="bg-muted text-foreground max-h-48 overflow-y-auto rounded-md p-2 font-mono text-[11px] leading-tight whitespace-pre-wrap"
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
