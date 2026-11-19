import { createEffect, createMemo, createSignal, onSettled, untrack } from "solid-js";
import type { Accessor } from "solid-js";
import type {
  AgentCapabilitiesPreview,
  AgentSummary,
  ConfigOptionValue,
  InitialConfig,
  NodeSummary,
  ProjectSummary,
  ProvisionerSummary,
  SessionSummary,
} from "#lib/generated/bindings.ts";
import { agentsAvailableOn } from "#lib/agents.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import { isModelOption, orderedConfigOptions, resolveInitialConfig } from "#lib/config-options.ts";
import { buildPathCandidates, pickDefaultCandidate, pickDefaultNode } from "#lib/new-session.ts";
import { otherNodesWithPaths } from "./new-session-candidates.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import type { SessionModePreference } from "#lib/session-prefs.ts";
import { problems } from "#lib/stores/problems.ts";

/**
 * New-session target: which project, node / provisioner, working directory and
 * agent a session starts on.
 *
 * Every default is derived with a memo instead of being written from an
 * effect, and a handler that has just written a signal passes the settled
 * value as an argument: Solid 2 commits signal writes on the next flush, so
 * reading a signal back in the same turn returns the previous value.
 */

/** Whether the session runs on a resident node or in an ephemeral VM. */
export type NewSessionRunTarget = "node" | "provisioner";

/** Working directory choice on the new-session screen. */
export type NewSessionPathMode = "existing" | "new_worktree";

/** Deep-link values, read once when the screen is created. */
export interface NewSessionInitial {
  /** `?project=` */
  project: string;
  /** `?node=` */
  node: string;
  /** `?agent=` */
  agent: string;
  /** `?path=` */
  path: string;
}

/** Session preferences remembered per project on this device. */
export interface NewSessionPrefs {
  agent: string;
  mode: SessionModePreference;
}

export interface NewSessionTargetOptions {
  /** Project explicitly chosen by the user (empty = deep link or first project). */
  chosenProjectId: Accessor<string>;
  /** Node explicitly chosen by the user (`null` = follow the default resolution). */
  chosenNodeId: Accessor<string | null>;
  /** Whether the deep-linked node still applies (cleared once the user changes project). */
  deepLinkNodeActive: Accessor<boolean>;
  /** Working directory explicitly chosen by the user (empty = default candidate). */
  chosenPath: Accessor<string>;
  /** A typed / browsed path, kept even when it is not a known candidate. */
  useCustomPath: Accessor<boolean>;
  /** Agent explicitly chosen by the user (empty = remembered / first available). */
  chosenAgentId: Accessor<string>;
  /** Manual agent-ID input (`null` = follow the catalog). */
  manualAgentChoice: Accessor<boolean | null>;
  runTarget: Accessor<NewSessionRunTarget>;
  provisionerName: Accessor<string>;
  setProvisionerName: (name: string) => void;
  pathMode: Accessor<NewSessionPathMode>;
  initial: NewSessionInitial;
  projects: Accessor<ProjectSummary[]>;
  nodes: Accessor<NodeSummary[]>;
  /** Session history for path candidates and default-node recency. */
  sessions: Accessor<readonly SessionSummary[]>;
  wsConnected: Accessor<boolean>;
  /** Remembered preferences for a project (local device only). */
  loadPrefs: (projectId: string) => NewSessionPrefs | null;
  /** Remembered initial-config values for one agent. */
  loadConfig: (projectId: string, agentId: string) => Record<string, ConfigOptionValue>;
  saveConfigOption: (
    projectId: string,
    agentId: string,
    key: string,
    value: ConfigOptionValue,
  ) => void;
  /** Provisioners available on the hub (a local node has none). */
  fetchProvisioners: () => Promise<ProvisionerSummary[]>;
  /** Agent catalog of one node (`GET /api/v1/nodes/{id}/agents`). */
  fetchAgents: (nodeId: string) => Promise<AgentSummary[]>;
  /** Capability probe for one node / agent / working directory. */
  probeAgent: (
    nodeId: string,
    agentId: string,
    path: string,
    force: boolean,
    initialConfig: InitialConfig[],
  ) => Promise<AgentCapabilitiesPreview>;
}

/** Working directory used to probe the agent (empty when none applies). */
function probePathFor(
  isProvisionerMode: boolean,
  mode: NewSessionPathMode,
  localPath: string,
  candidates: ReturnType<typeof buildPathCandidates>,
): string {
  if (isProvisionerMode) return "";
  if (mode === "existing") return localPath;
  return candidates.find((candidate) => candidate.exists)?.path ?? candidates[0]?.path ?? "";
}

/**
 * Model selection applied to the capability probe.
 *
 * Some agents publish dependent options such as `effort` only after a model is
 * selected. Before the first probe the only model value known is the
 * conventional `model` remembered for this project and agent. It is scoped by
 * agent because model values are agent-specific (OpenCode expects
 * `provider/model`, Gemini CLI uses bare IDs).
 *
 * Takes the preview, the selection and the remembered model as arguments: a
 * caller that has just written the signals passes the new values instead of
 * reading them back before the flush.
 */
function configForProbe(
  preview: AgentCapabilitiesPreview | null,
  selectedValues: Record<string, ConfigOptionValue>,
  savedModel: ConfigOptionValue | undefined,
): InitialConfig[] {
  if (preview !== null) {
    const option = (preview.config_options ?? []).find((candidate) => isModelOption(candidate));
    if (option === undefined) return [];
    const value = selectedValues[option.key];
    return value === undefined ? [] : [{ key: option.key, value }];
  }
  return savedModel === undefined ? [] : [{ key: "model", value: savedModel }];
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

/** Whether two initial-config lists describe the same probe input. */
function sameConfig(left: InitialConfig[], right: InitialConfig[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/** UI cache key for one probe request (agent / path / initial config). */
function probeCacheKey(key: string, config: InitialConfig[]): string {
  return `${key}\u0000${JSON.stringify(config)}`;
}

/**
 * Project / node / path / agent resolution and the capability probe for the
 * new-session screen.
 */
export function createNewSessionTarget(options: NewSessionTargetOptions) {
  const {
    chosenProjectId,
    chosenNodeId,
    deepLinkNodeActive,
    chosenPath,
    useCustomPath,
    chosenAgentId,
    manualAgentChoice,
    runTarget,
    provisionerName,
    setProvisionerName,
    pathMode,
    initial,
    projects,
    nodes,
    sessions,
    wsConnected,
    loadPrefs,
    loadConfig,
    saveConfigOption,
  } = options;

  const isProvisionerMode = () => runTarget() === "provisioner";

  const [provisioners, setProvisioners] = createSignal<ProvisionerSummary[]>([]);
  const [provisionerLoadError, setProvisionerLoadError] = createSignal<string | null>(null);
  /** Agents reported for `catalogNodeId` (never mixed between nodes). */
  const [catalog, setCatalog] = createSignal<AgentSummary[]>([]);
  const [catalogLoading, setCatalogLoading] = createSignal(true);
  const [catalogError, setCatalogError] = createSignal<string | null>(null);
  /** Node whose catalog is currently loaded (never mix two nodes' lists). */
  const [catalogNodeId, setCatalogNodeId] = createSignal("");
  /** Guards against stale responses when the node changes / the user retries. */
  let catalogGeneration = 0;

  // Agent capability probe (models / modes)
  const [capabilities, setCapabilities] = createSignal<AgentCapabilitiesPreview | null>(null);
  const [capabilitiesKey, setCapabilitiesKey] = createSignal("");
  const [capabilitiesLoading, setCapabilitiesLoading] = createSignal(false);
  const [capabilitiesError, setCapabilitiesError] = createSignal<string | null>(null);
  let capabilitiesGeneration = 0;
  /** Per-screen probe cache so re-selecting the same agent is instant. */
  const capabilitiesCache = new Map<string, AgentCapabilitiesPreview>();
  const [selectedConfigValues, setSelectedConfigValues] = createSignal<
    Record<string, ConfigOptionValue>
  >({});

  const selectedProject = createMemo(
    () =>
      projects().find((project) => project.project_id === chosenProjectId()) ??
      projects().find((project) => project.project_id === initial.project) ??
      projects()[0] ??
      null,
  );
  const onlineNodes = () => nodes().filter((node) => node.is_online);
  /** Deep-link node, only once it exists in the current node list. */
  const deepLinkNodeId = () =>
    deepLinkNodeActive() &&
    initial.node !== "" &&
    nodes().some((node) => node.node_id === initial.node)
      ? initial.node
      : "";
  const defaultNodeId = createMemo(() => {
    const list = nodes();
    if (list.length === 0) return "";
    const deepLink = deepLinkNodeId();
    if (deepLink !== "") return deepLink;
    // Read the project and the node list reactively (so a project / node change
    // re-resolves the default), but not session history: session activity
    // reorders candidates and must not move the selection.
    const project = selectedProject();
    return pickDefaultNode(
      list,
      project,
      untrack(() => sessions()),
    );
  });
  const nodeId = createMemo(() => {
    const chosen = chosenNodeId();
    return chosen !== null && nodes().some((node) => node.node_id === chosen)
      ? chosen
      : defaultNodeId();
  });
  const selectedNode = createMemo(() => nodes().find((node) => node.node_id === nodeId()) ?? null);
  const nodeAvailability = () => getNodeAvailability(selectedNode(), wsConnected());
  /** Whether `catalog` belongs to the current node (never mix two nodes' lists). */
  const catalogReady = () => nodeId() !== "" && catalogNodeId() === nodeId();
  // Agents that can actually be launched on this node (based on the catalog's
  // installed_nodes; npx/uvx distributed agents are included only once enabled
  // via "add from catalog")
  const availableAgents = () => (catalogReady() ? agentsAvailableOn(catalog(), nodeId()) : []);
  const availableAgentIds = () => availableAgents().map((agent) => agent.id);
  /** Remembered agent for the project, as long as this node still offers it. */
  const rememberedAgent = createMemo(() => {
    const projectId = selectedProject()?.project_id ?? "";
    if (projectId === "" || !catalogReady()) return "";
    const saved = loadPrefs(projectId);
    return saved !== null && availableAgentIds().includes(saved.agent) ? saved.agent : "";
  });
  const effectiveAgent = () => {
    const chosen = chosenAgentId().trim();
    if (chosen !== "") return chosen;
    const remembered = rememberedAgent();
    return remembered !== "" ? remembered : (availableAgentIds()[0] ?? "");
  };
  const selectedAgent = createMemo(
    () => availableAgents().find((agent) => agent.id === effectiveAgent()) ?? null,
  );
  /**
   * Whether to show the free-form agent ID input instead of the catalog list.
   * Until the user (or a selection) fixes the choice, a deep-linked agent that
   * the loaded catalog does not contain starts in manual mode.
   */
  const manualAgent = () =>
    manualAgentChoice() ?? (catalogReady() && !availableAgentIds().includes(initial.agent));

  const selectedProvisioner = createMemo(
    () => provisioners().find((p) => p.name === provisionerName()) ?? provisioners()[0] ?? null,
  );

  /** Loads ephemeral VM provisioners (hub only; retryable from the UI). */
  async function loadProvisioners(): Promise<void> {
    try {
      const list = await options.fetchProvisioners();
      setProvisioners(list);
      setProvisionerLoadError(null);
      if (list.length > 0 && provisionerName() === "") setProvisionerName(list[0].name);
    } catch (error) {
      // A local node has no provisioners, so treat this as a warning rather than
      // a hard failure; the user can still start resident-node sessions.
      setProvisionerLoadError(errorMessage(error));
      problems.report("load provisioners", error, "warning");
    }
  }

  /**
   * Loads the agent catalog for one node.
   *
   * The hub rejects the unscoped `GET /api/v1/agents` (every catalog is
   * node-scoped), so always fetch through `/api/v1/nodes/{id}/agents`. The user
   * can keep going by entering an ID directly even if the catalog cannot be
   * fetched.
   */
  async function loadAgentCatalog(target: string): Promise<void> {
    if (target === "") return;
    // Only the newest request may publish (node switches / retries can overlap)
    const generation = ++catalogGeneration;
    setCatalogLoading(true);
    try {
      const list = await options.fetchAgents(target);
      if (generation !== catalogGeneration) return;
      setCatalog(list);
      setCatalogNodeId(target);
      setCatalogError(null);
    } catch (error) {
      if (generation !== catalogGeneration) return;
      setCatalog([]);
      setCatalogNodeId(target);
      setCatalogError(errorMessage(error));
      problems.report("load agents", error);
    } finally {
      if (generation === catalogGeneration) setCatalogLoading(false);
    }
  }

  // Ephemeral VM provisioner definitions (only available when connected to the hub)
  onSettled(() => {
    void loadProvisioners();
  });

  // The agent catalog is node-scoped: reload it when the target node changes.
  createEffect(
    () => nodeId(),
    (target) => {
      if (target === "") {
        // No node to query (e.g. the hub has no connected nodes): stop the loading state.
        setCatalogLoading(false);
        return;
      }
      void loadAgentCatalog(target);
    },
  );

  // Resolve working directory candidates (bindings + recent sessions) in last-used order
  const pathCandidates = createMemo(() =>
    buildPathCandidates(selectedProject(), nodeId(), sessions()),
  );
  /**
   * Pin the automatic path per project × node (and its empty → ready
   * transition). Session updates reorder candidates but must not replace the
   * directory the user is about to use.
   */
  const autoPathKey = createMemo(
    () =>
      `${selectedProject()?.project_id ?? ""}\u0000${nodeId()}\u0000${pathCandidates().length === 0 ? "empty" : "ready"}`,
  );
  const defaultPath = createMemo(() => {
    void autoPathKey();
    return untrack(() => pickDefaultCandidate(pathCandidates())?.path ?? "");
  });
  /** Selected working directory (explicit choice, or the pinned default). */
  const localPath = createMemo(() => {
    // A typed / browsed path is used as-is; a picked candidate only while it
    // still exists in the list (otherwise fall back to the default).
    const custom = chosenPath();
    if (custom !== "" && (useCustomPath() || pathCandidates().some((c) => c.path === custom))) {
      return custom;
    }
    const automatic = defaultPath();
    if (automatic !== "" && pathCandidates().some((candidate) => candidate.path === automatic)) {
      return automatic;
    }
    return pickDefaultCandidate(pathCandidates())?.path ?? "";
  });
  const selectedCandidate = createMemo(
    () => pathCandidates().find((candidate) => candidate.path === localPath()) ?? null,
  );
  // Alternative nodes when this node has no candidates (another node with bindings)
  const otherNodesWithCandidates = createMemo(() =>
    otherNodesWithPaths(
      nodes(),
      nodeId(),
      (candidateNodeId) =>
        buildPathCandidates(selectedProject(), candidateNodeId, sessions()).length > 0,
    ),
  );

  /** Working directory used to probe the agent for models. */
  const probePath = () =>
    probePathFor(isProvisionerMode(), pathMode(), localPath(), pathCandidates());

  /** Remembered model value for the current project / agent (no preview yet). */
  const savedModel = createMemo(() => {
    const projectId = selectedProject()?.project_id ?? "";
    const agent = effectiveAgent();
    if (projectId === "" || agent === "") return undefined;
    return loadConfig(projectId, agent)["model"];
  });

  /**
   * Resolve the initial config for a probe result, keeping values that are
   * still valid for this agent.
   */
  function applyCapabilities(
    key: string,
    preview: AgentCapabilitiesPreview,
    probedConfig: InitialConfig[],
  ): void {
    setCapabilities(preview);
    setCapabilitiesKey(key);
    setCapabilitiesLoading(false);
    const projectId = selectedProject()?.project_id ?? "";
    const agent = effectiveAgent();
    const saved = projectId === "" || agent === "" ? {} : loadConfig(projectId, agent);
    const resolved = resolveInitialConfig(preview.config_options ?? [], saved);
    // Replacing the object invalidates every reader even when the selection did
    // not change. The probe effect applies cached results synchronously, so a
    // needless replacement there would make the effect re-run forever.
    if (!sameConfigMap(resolved, selectedConfigValues())) {
      setSelectedConfigValues(resolved);
    }

    // The probe may have used no model (first visit) or a remembered model that
    // is no longer available. Probe once more with the model actually selected
    // so dependent options such as effort are included. Read the freshly
    // resolved selection: the signal write above is not visible yet.
    const selectedModelConfig = configForProbe(preview, resolved, undefined);
    if (selectedModelConfig.length > 0 && !sameConfig(selectedModelConfig, probedConfig)) {
      void loadAgentCapabilities(false, selectedModelConfig);
    }
  }

  async function loadAgentCapabilities(
    force = false,
    config: InitialConfig[] | undefined = undefined,
  ): Promise<void> {
    const initialConfigs =
      config ?? configForProbe(capabilities(), selectedConfigValues(), savedModel());
    const key = probeKey();
    if (key === "") {
      capabilitiesGeneration += 1;
      setCapabilities(null);
      setCapabilitiesKey("");
      setCapabilitiesLoading(false);
      setCapabilitiesError(null);
      setSelectedConfigValues({});
      return;
    }
    const cacheKey = probeCacheKey(key, initialConfigs);
    if (!force) {
      const cached = capabilitiesCache.get(cacheKey);
      if (cached) {
        applyCapabilities(key, cached, initialConfigs);
        return;
      }
    }
    const generation = ++capabilitiesGeneration;
    setCapabilitiesLoading(true);
    setCapabilitiesError(null);
    try {
      const preview = await options.probeAgent(
        nodeId(),
        effectiveAgent(),
        probePath(),
        force,
        initialConfigs,
      );
      if (generation !== capabilitiesGeneration) return;
      capabilitiesCache.set(cacheKey, preview);
      applyCapabilities(key, preview, initialConfigs);
    } catch (error) {
      if (generation !== capabilitiesGeneration) return;
      setCapabilities(null);
      setCapabilitiesKey(key);
      setSelectedConfigValues({});
      setCapabilitiesError(errorMessage(error));
      problems.report("probe agent", error, "warning");
    } finally {
      if (generation === capabilitiesGeneration) setCapabilitiesLoading(false);
    }
  }

  /** Stable key for the probe target (node / agent / working directory). */
  const probeKey = createMemo(() =>
    isProvisionerMode() ||
    !catalogReady() ||
    !nodeAvailability().isAvailable ||
    nodeId() === "" ||
    effectiveAgent() === "" ||
    probePath() === ""
      ? ""
      : `${nodeId()}\u0000${effectiveAgent()}\u0000${probePath()}`,
  );

  // Probe when the node / agent / working directory selection changes.
  createEffect(
    () => probeKey(),
    (key) => {
      if (key === "") {
        capabilitiesGeneration += 1;
        setCapabilities(null);
        setCapabilitiesKey("");
        setCapabilitiesLoading(false);
        setCapabilitiesError(null);
        setSelectedConfigValues({});
        return;
      }
      // Drop the previous agent's / path's options immediately; the new probe
      // starts from a clean loading state.
      setCapabilities(null);
      setCapabilitiesKey("");
      setCapabilitiesLoading(true);
      setCapabilitiesError(null);
      setSelectedConfigValues({});
      // Probe with the config saved for the target agent, not with the
      // previous agent's signals (they are about to be replaced).
      const config = configForProbe(null, {}, savedModel());
      // Apply a cached probe inside `untrack`: the cache-hit path runs
      // synchronously and writes `capabilities` / `selectedConfigValues`. If the
      // effect subscribed to those writes it would re-run, take the same cache
      // hit again and loop.
      untrack(() => {
        void loadAgentCapabilities(false, config);
      });
    },
  );

  // Only show options that belong to the current selection. This prevents the
  // previous agent's model list from flashing while the next probe is running.
  const configOptions = createMemo(() =>
    capabilitiesKey() === probeKey()
      ? orderedConfigOptions(capabilities()?.config_options ?? [])
      : [],
  );

  function changeConfigOption(key: string, value: ConfigOptionValue): void {
    setSelectedConfigValues((current) => ({ ...current, [key]: value }));
    const projectId = selectedProject()?.project_id ?? "";
    const agent = effectiveAgent();
    if (projectId !== "" && agent !== "") {
      saveConfigOption(projectId, agent, key, value);
    }

    // Selecting a model can reveal dependent options (for example effort
    // levels) that the previous probe did not return.
    const option = (capabilities()?.config_options ?? []).find(
      (candidate) => candidate.key === key,
    );
    if (option !== undefined && isModelOption(option)) {
      void loadAgentCapabilities(true, [{ key, value }]);
    }
  }

  function initialConfig(): InitialConfig[] {
    // Only send options that the current agent reported and that were validated
    // against its choices. Without a completed probe we cannot tell whether a
    // remembered value still exists for this agent (or even belongs to it), and
    // the agent rejects the whole `set_config_option` request when a model is
    // unknown. A missing value is better than a failed application: the agent
    // then keeps its own default.
    if (probeKey() === "" || capabilities() === null || capabilitiesKey() !== probeKey()) return [];
    return Object.entries(selectedConfigValues()).map(([key, value]) => ({ key, value }));
  }

  /** Reload the catalog for the current node (the error card's retry). */
  function reloadAgentCatalog(): Promise<void> {
    return loadAgentCatalog(nodeId());
  }

  return {
    isProvisionerMode,
    selectedProject,
    onlineNodes,
    nodeId,
    selectedNode,
    nodeAvailability,
    catalogLoading,
    catalogError,
    availableAgents,
    effectiveAgent,
    selectedAgent,
    manualAgent,
    selectedProvisioner,
    provisioners,
    provisionerLoadError,
    loadProvisioners,
    pathCandidates,
    localPath,
    selectedCandidate,
    otherNodesWithCandidates,
    probePath,
    capabilitiesLoading,
    capabilitiesError,
    /** A probe target exists (node / agent / path resolved and available). */
    probeTarget: () => probeKey() !== "",
    /** Whether the probe results belong to the current node / agent / path. */
    probeReady: () => probeKey() !== "" && capabilitiesKey() === probeKey(),
    /** The probe is running while the current results are stale or missing. */
    probePending: () => {
      const key = probeKey();
      return capabilitiesLoading() || (probeKey() !== "" && capabilitiesKey() !== key);
    },
    configOptions,
    /** Modes reported by the probed agent (`null` while it has not answered yet). */
    availableModes: createMemo(() => {
      const preview = capabilities();
      return capabilitiesKey() === probeKey() && preview !== null
        ? (preview.available_modes ?? [])
        : null;
    }),
    changeConfigOption,
    initialConfig,
    /** Selected config values (model / effort / ...) for the probe target. */
    selectedConfigValues,
    /** Whether a probe result for the current target is available. */
    capabilitiesReady: () => capabilities() !== null,
    /** Capability probe with `force` (the picker's refresh button). */
    refreshCapabilities: () => loadAgentCapabilities(true),
    /** Reload the catalog for the current node (the error card's retry). */
    reloadAgentCatalog,
  };
}
