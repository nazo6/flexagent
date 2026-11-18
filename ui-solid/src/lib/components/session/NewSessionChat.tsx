import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { useNavigate, useSearchParams } from "@solidjs/router";
import AgentIcon from "#lib/components/agent/AgentIcon.tsx";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.tsx";
import ComposerCard from "#lib/components/composer/ComposerCard.tsx";
import ComposerMoreButton from "#lib/components/composer/ComposerMoreButton.tsx";
import ComposerSendButton from "#lib/components/composer/ComposerSendButton.tsx";
import ComposerTextarea from "#lib/components/composer/ComposerTextarea.tsx";
import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.tsx";
import ModePicker from "#lib/components/composer/ModePicker.tsx";
import PickerItem from "#lib/components/composer/PickerItem.tsx";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import { createComposerAttachments } from "#lib/composer-attachments.ts";
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
import { randomUuid } from "#lib/uuid.ts";
import { isModelOption, orderedConfigOptions, resolveInitialConfig } from "#lib/config-options.ts";
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
import { prefs } from "#lib/stores/prefs.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import { nodeSettingsPath, resolveDefaultNodeId } from "#lib/settings-routes.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { problems } from "#lib/stores/problems.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconBox from "~icons/lucide/box";
import IconCheckCircle2 from "~icons/lucide/check-circle-2";
import IconFile from "~icons/lucide/file";
import IconFolderGit2 from "~icons/lucide/folder-git-2";
import IconGitBranch from "~icons/lucide/git-branch";
import IconGitFork from "~icons/lucide/git-fork";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPaperclip from "~icons/lucide/paperclip";
import IconServer from "~icons/lucide/server";
import IconSparkles from "~icons/lucide/sparkles";
import IconX from "~icons/lucide/x";
import IconXCircle from "~icons/lucide/x-circle";

type PathMode = "existing" | "new_worktree";
type RunTarget = "node" | "provisioner";

/**
 * New-session screen.
 *
 * Deep-link parameters are read once when the screen is created. The URL is an
 * entry point, not a reactive input: applying it again while the user edits the
 * form would overwrite their selections (`untrack`).
 */
export default function NewSessionChat() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams<{
    project?: string;
    node?: string;
    path?: string;
    fork_session?: string;
    fork_upto?: string;
    agent?: string;
  }>();

  const initialUrl = untrack(() => {
    const forkUptoRaw = searchParams.fork_upto ?? null;
    const forkUpto = forkUptoRaw === null ? Number.NaN : Number(forkUptoRaw);
    return {
      project: searchParams.project ?? "",
      node: searchParams.node ?? "",
      path: searchParams.path ?? "",
      forkSession: searchParams.fork_session ?? "",
      forkUpTo: Number.isSafeInteger(forkUpto) && forkUpto >= 0 ? forkUpto : null,
      agent: searchParams.agent ?? "",
    };
  });

  /** Project explicitly chosen by the user (empty = deep link or first project). */
  const [chosenProjectId, setChosenProjectId] = createSignal("");
  /** Node explicitly chosen by the user (`null` = follow the default resolution). */
  const [chosenNodeId, setChosenNodeId] = createSignal<string | null>(null);
  /** Whether the deep-linked node still applies (cleared once the user changes project). */
  const [deepLinkNodeActive, setDeepLinkNodeActive] = createSignal(initialUrl.node !== "");
  /** Working directory explicitly chosen by the user (empty = default candidate). */
  const [chosenPath, setChosenPath] = createSignal(initialUrl.path);
  /** Agent explicitly chosen by the user (empty = remembered / first available). */
  const [chosenAgentId, setChosenAgentId] = createSignal(initialUrl.agent);
  /**
   * Initial mode explicitly chosen by the user (`null` = remembered mode).
   * A deep-linked agent starts from its own default because mode IDs are
   * agent-specific.
   */
  const [chosenMode, setChosenMode] = createSignal<SessionModePreference | null>(
    initialUrl.agent === "" ? null : "default",
  );
  /**
   * Manual agent-ID input (`null` = follow the catalog).
   *
   * A deep link to an agent missing from the catalog starts in manual mode; the
   * user's toggle (or a catalog selection) fixes the choice afterwards.
   */
  const [manualAgentChoice, setManualAgentChoice] = createSignal<boolean | null>(
    initialUrl.agent === "" ? false : null,
  );

  const [runTarget, setRunTarget] = createSignal<RunTarget>("node");
  const [provisionerName, setProvisionerName] = createSignal("");
  const [provisioners, setProvisioners] = createSignal<ProvisionerSummary[]>([]);
  /** ACP Registry catalog + per-node availability. */
  const [catalog, setCatalog] = createSignal<AgentSummary[]>([]);
  const [catalogLoading, setCatalogLoading] = createSignal(true);
  const [catalogError, setCatalogError] = createSignal<string | null>(null);
  /** Node whose catalog is currently loaded (never mix two nodes' lists). */
  const [catalogNodeId, setCatalogNodeId] = createSignal("");
  /** Guards against stale responses when the node changes / the user retries. */
  let catalogGeneration = 0;
  const [provisionerLoadError, setProvisionerLoadError] = createSignal<string | null>(null);
  const [pathMode, setPathMode] = createSignal<PathMode>("existing");
  const [branch, setBranch] = createSignal("");
  const [baseBranch, setBaseBranch] = createSignal("");
  const draft = composerDrafts.get(NEW_SESSION_DRAFT_ID);
  const [promptText, setPromptText] = createSignal(draft.text);
  const [busy, setBusy] = createSignal(false);
  const [browserOpen, setBrowserOpen] = createSignal(false);
  const [fileBrowserOpen, setFileBrowserOpen] = createSignal(false);
  const [fileInput, setFileInput] = createSignal<HTMLInputElement>();
  const [useCustomPath, setUseCustomPath] = createSignal(initialUrl.path !== "");

  const composer = createComposerAttachments(draft.attachments);
  const attachments = () => composer.attachments;

  // Keep the prompt while the user visits another screen and comes back.
  createEffect(
    () => promptText(),
    (value) => composerDrafts.setText(NEW_SESSION_DRAFT_ID, value),
  );

  createEffect(
    () => attachments(),
    (value) => composerDrafts.setAttachments(NEW_SESSION_DRAFT_ID, value),
  );

  // Picker popovers
  const [projectOpen, setProjectOpen] = createSignal(false);
  const [directoryOpen, setDirectoryOpen] = createSignal(false);
  const [environmentOpen, setEnvironmentOpen] = createSignal(false);
  const [agentOpen, setAgentOpen] = createSignal(false);
  const [moreOpen, setMoreOpen] = createSignal(false);

  // Fork inheritance
  const [forkSessionId, setForkSessionId] = createSignal(initialUrl.forkSession);
  const [forkUpToNodeSeq, setForkUpToNodeSeq] = createSignal<number | null>(initialUrl.forkUpTo);

  // Launch options
  const [extraArgsText, setExtraArgsText] = createSignal("");

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

  // Provisioner connectivity test
  const [testBusy, setTestBusy] = createSignal(false);
  const [testResult, setTestResult] = createSignal<ProvisionerTestResponse | null>(null);
  const [testDialogOpen, setTestDialogOpen] = createSignal(false);

  const projects = () => sync.projects;
  const selectedProject = createMemo(
    () =>
      projects().find((project) => project.project_id === chosenProjectId()) ??
      projects().find((project) => project.project_id === initialUrl.project) ??
      projects()[0] ??
      null,
  );

  const isProvisionerMode = () => runTarget() === "provisioner";
  const nodes = () => sync.nodes;
  const onlineNodes = () => nodes().filter((node) => node.is_online);
  /** Deep-link node, only once it exists in the current node list. */
  const deepLinkNodeId = () =>
    deepLinkNodeActive() &&
    initialUrl.node !== "" &&
    nodes().some((node) => node.node_id === initialUrl.node)
      ? initialUrl.node
      : "";
  const defaultNodeId = createMemo(() => {
    const list = nodes();
    if (list.length === 0) return "";
    const deepLink = deepLinkNodeId();
    if (deepLink !== "") return deepLink;
    // Read the project and node list reactively (so a project / node change
    // re-resolves the default), but not session history: session activity
    // reorders candidates and must not move the selection.
    const project = selectedProject();
    return pickDefaultNode(
      list,
      project,
      untrack(() => sync.sessions),
    );
  });
  const nodeId = createMemo(() => {
    const chosen = chosenNodeId();
    return chosen !== null && nodes().some((node) => node.node_id === chosen)
      ? chosen
      : defaultNodeId();
  });
  const selectedNode = createMemo(() => nodes().find((node) => node.node_id === nodeId()) ?? null);
  const nodeAvailability = () => getNodeAvailability(selectedNode(), sync.wsConnected);
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
    const saved = loadNewSessionPrefs(projectId);
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
    manualAgentChoice() ??
    (catalogReady() && !availableAgentIds().includes(initialUrl.agent));

  const agentsSettingsHref = createMemo(() =>
    nodeSettingsPath(
      "agents",
      (!isProvisionerMode() && nodeId() !== "" ? nodeId() : null) ??
        resolveDefaultNodeId(
          sync.connection.role === "hub",
          nodes(),
          sync.connection.systemInfo?.node_id,
        ),
    ),
  );
  const selectedProvisioner = createMemo(
    () => provisioners().find((p) => p.name === provisionerName()) ?? provisioners()[0] ?? null,
  );

  /** Loads ephemeral VM provisioners (hub only; retryable from the UI). */
  async function loadProvisioners(): Promise<void> {
    try {
      const list = await sync.connection.client.provisioners();
      setProvisioners(list);
      setProvisionerLoadError(null);
      if (list.length > 0 && provisionerName() === "") setProvisionerName(list[0].name);
    } catch (error) {
      // A local node has no provisioners, so treat this as a warning rather than
      // a hard failure; the user can still start resident-node sessions.
      setProvisionerLoadError(error instanceof Error ? error.message : String(error));
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
    setCatalogLoading(true);
    try {
      const list = await sync.connection.client.nodeAgents(target);
      if (generation !== catalogGeneration) return;
      setCatalog(list);
      setCatalogNodeId(target);
      setCatalogError(null);
    } catch (error) {
      if (generation !== catalogGeneration) return;
      setCatalog([]);
      setCatalogNodeId(target);
      setCatalogError(error instanceof Error ? error.message : String(error));
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
    buildPathCandidates(selectedProject(), nodeId(), sync.sessions),
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
    nodes().filter(
      (node) =>
        node.node_id !== nodeId() &&
        buildPathCandidates(selectedProject(), node.node_id, sync.sessions).length > 0,
    ),
  );

  /** Working directory used to probe the agent for models. */
  const probePath = () => {
    if (isProvisionerMode()) return "";
    if (pathMode() === "existing") return localPath();
    return (
      pathCandidates().find((candidate) => candidate.exists)?.path ??
      pathCandidates()[0]?.path ??
      ""
    );
  };

  /**
   * Model selection applied to the capability probe.
   *
   * Some agents publish dependent options such as `effort` only after a model
   * is selected. Before the first probe the only model key we know is the
   * conventional `model` remembered for this project and agent. It is scoped by
   * agent because model values are agent-specific (OpenCode expects
   * `provider/model`, Gemini CLI uses bare IDs).
   */
  function currentModelConfig(): InitialConfig[] {
    const preview = capabilities();
    if (preview !== null) {
      const option = (preview.config_options ?? []).find((candidate) => isModelOption(candidate));
      if (option === undefined) return [];
      const value = selectedConfigValues()[option.key];
      return value === undefined ? [] : [{ key: option.key, value }];
    }
    const projectId = selectedProject()?.project_id ?? "";
    if (projectId === "" || effectiveAgent() === "") return [];
    const saved = loadSessionConfig(projectId, effectiveAgent())["model"];
    return saved === undefined ? [] : [{ key: "model", value: saved }];
  }

  /** UI cache key for one probe request (agent / path / initial config). */
  function probeCacheKey(key: string, config: InitialConfig[]): string {
    return `${key}\u0000${JSON.stringify(config)}`;
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

  // Only show options that belong to the current selection. This prevents the
  // previous agent's model list from flashing while the next probe is running.
  const configOptions = createMemo(() =>
    capabilitiesKey() === probeKey() ? orderedConfigOptions(capabilities()?.config_options ?? []) : [],
  );

  /**
   * Modes reported by the probed agent (`null` while it has not answered yet).
   *
   * The initial-mode choices come from the agent instead of a fixed list, so
   * agent-specific IDs (OpenCode's `build`, for example) are passed through
   * unchanged.
   */
  const availableModes = createMemo<ModeInfo[] | null>(() => {
    const preview = capabilities();
    return capabilitiesKey() === probeKey() && preview !== null
      ? (preview.available_modes ?? [])
      : null;
  });

  /** Remembered initial mode for the project (agent-specific IDs are not mixed). */
  const rememberedMode = createMemo((): SessionModePreference => {
    const projectId = selectedProject()?.project_id ?? "";
    if (projectId === "" || !catalogReady()) return "default";
    return loadNewSessionPrefs(projectId)?.mode ?? "default";
  });

  /**
   * Initial mode sent with the session. A remembered mode the probed agent does
   * not offer is ignored instead of failing `session/set_mode`.
   */
  const sessionMode = createMemo((): SessionModePreference => {
    const proposed = chosenMode() ?? rememberedMode();
    const modes = availableModes();
    if (proposed === "default" || modes === null) return proposed;
    return modes.some((mode) => mode.mode_id === proposed) ? proposed : "default";
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
    setCapabilities(preview);
    setCapabilitiesKey(key);
    setCapabilitiesLoading(false);
    const projectId = selectedProject()?.project_id ?? "";
    const saved =
      projectId === "" || effectiveAgent() === ""
        ? {}
        : loadSessionConfig(projectId, effectiveAgent());
    const resolved = resolveInitialConfig(preview.config_options ?? [], saved);
    // Replacing the object invalidates every reader even when the selection did
    // not change. The probe effect applies cached results synchronously, so a
    // needless replacement there would make the effect re-run forever.
    if (!sameConfigMap(resolved, selectedConfigValues())) {
      setSelectedConfigValues(resolved);
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
    const cacheKey = probeCacheKey(key, config);
    if (!force) {
      const cached = capabilitiesCache.get(cacheKey);
      if (cached) {
        applyCapabilities(key, cached, config);
        return;
      }
    }
    const generation = ++capabilitiesGeneration;
    setCapabilitiesLoading(true);
    setCapabilitiesError(null);
    try {
      const preview = await sync.connection.client.probeAgent(
        nodeId(),
        effectiveAgent(),
        probePath(),
        force,
        config,
      );
      if (generation !== capabilitiesGeneration) return;
      capabilitiesCache.set(cacheKey, preview);
      applyCapabilities(key, preview, config);
    } catch (error) {
      if (generation !== capabilitiesGeneration) return;
      setCapabilities(null);
      setCapabilitiesKey(key);
      setSelectedConfigValues({});
      setCapabilitiesError(error instanceof Error ? error.message : String(error));
      problems.report("probe agent", error, "warning");
    } finally {
      if (generation === capabilitiesGeneration) setCapabilitiesLoading(false);
    }
  }

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
      // Drop the previous agent's/path's options immediately; the new probe
      // starts from a clean loading state.
      setCapabilities(null);
      setCapabilitiesKey("");
      setCapabilitiesLoading(true);
      setCapabilitiesError(null);
      setSelectedConfigValues({});
      // Apply a cached probe inside `untrack`: the cache-hit path runs
      // synchronously and writes `capabilities` / `selectedConfigValues`. If the
      // effect subscribed to those writes it would re-run, take the same cache
      // hit again and loop.
      untrack(() => {
        void loadAgentCapabilities(false, currentModelConfig());
      });
    },
  );

  function changeConfigOption(key: string, value: ConfigOptionValue): void {
    setSelectedConfigValues((current) => ({ ...current, [key]: value }));
    const projectId = selectedProject()?.project_id ?? "";
    if (projectId !== "" && effectiveAgent() !== "") {
      saveSessionConfigOption(projectId, effectiveAgent(), key, value);
    }

    // Selecting a model can reveal dependent options (for example effort
    // levels) that the previous probe did not return.
    const option = (capabilities()?.config_options ?? []).find((candidate) => candidate.key === key);
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
    if (probeKey() === "" || capabilities() === null || capabilitiesKey() !== probeKey()) return [];
    return Object.entries(selectedConfigValues()).map(([key, value]) => ({ key, value }));
  }

  const canSubmit = () =>
    !busy() &&
    selectedProject() !== null &&
    effectiveAgent() !== "" &&
    (isProvisionerMode()
      ? selectedProvisioner() !== null
      : nodeId() !== "" &&
        nodeAvailability().isAvailable &&
        (pathMode() === "existing" ? localPath() !== "" : branch().trim() !== ""));

  // Explain to the user why submit is unavailable (avoid a silent disabled state)
  const blockingHint = createMemo(() => {
    // The project callout above the composer already explains these states.
    if (sync.projectsLoading || projects().length === 0 || sync.projectsError !== null) return "";
    if (!isProvisionerMode()) {
      if (nodes().length === 0) return m.new_session_no_nodes();
      if (onlineNodes().length === 0) return m.new_session_no_online_nodes();
      const node = selectedNode();
      if (node !== null && !nodeAvailability().isAvailable) {
        return m.new_session_node_unavailable({
          node: node.name,
          status: nodeAvailability().statusText,
        });
      }
    } else if (selectedProvisioner() === null) {
      return m.new_session_provisioner_undefined();
    }
    if (effectiveAgent() === "") return m.new_session_agent_required();
    if (
      !isProvisionerMode() &&
      (pathMode() === "existing" ? localPath().trim() === "" : branch().trim() === "")
    ) {
      return pathMode() === "existing"
        ? m.new_session_path_required_existing()
        : m.new_session_path_required_branch();
    }
    return "";
  });

  const startTitle = () =>
    blockingHint() !== ""
      ? blockingHint()
      : promptText().trim() === ""
        ? m.new_session_start_empty_title()
        : m.new_session_start_title();

  function pathBasename(path: string): string {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : path;
  }

  const projectLabel = () => selectedProject()?.name ?? "";
  const directoryLabel = () =>
    pathMode() === "new_worktree"
      ? branch().trim() || m.new_session_new_worktree()
      : localPath().trim() !== ""
        ? pathBasename(localPath())
        : "";
  const environmentLabel = () =>
    isProvisionerMode()
      ? (selectedProvisioner()?.description ?? selectedProvisioner()?.name ?? provisionerName())
      : (selectedNode()?.name ?? "");
  const agentLabel = () => (manualAgent() ? effectiveAgent() : (selectedAgent()?.name ?? ""));

  async function runProvisionerTest(name: string): Promise<void> {
    if (testBusy() || !name) return;
    setEnvironmentOpen(false);
    setTestBusy(true);
    setTestResult(null);
    setTestDialogOpen(true);
    try {
      const result = await sync.connection.client.testProvisioner(name);
      setTestResult(result);
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
      setTestResult({
        name,
        ok: false,
        node_id: null,
        log_lines: [],
        error: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setTestBusy(false);
    }
  }

  async function handleStartSession(initialPrompt?: string): Promise<void> {
    const project = selectedProject();
    if (!canSubmit() || project === null) return;
    setBusy(true);
    const promptToSend = (initialPrompt ?? promptText()).trim();
    const useProvisioner = isProvisionerMode() && selectedProvisioner() !== null;
    const extraArgs = extraArgsText().trim() ? extraArgsText().trim().split(/\s+/) : null;
    // Remember on the local device for the next new session
    persistPrefs();

    try {
      const response = await sync.connection.client.createSession({
        command_id: randomUuid(),
        project_id: project.project_id,
        node_id: useProvisioner ? null : nodeId(),
        provisioner: useProvisioner ? (selectedProvisioner()?.name ?? null) : null,
        local_path: useProvisioner || pathMode() === "new_worktree" ? null : localPath(),
        worktree: useProvisioner
          ? branch().trim() === ""
            ? null
            : { branch: branch().trim(), base_branch: null }
          : pathMode() === "new_worktree"
            ? {
                branch: branch().trim(),
                base_branch: baseBranch().trim() === "" ? null : baseBranch().trim(),
              }
            : null,
        agent_id: effectiveAgent(),
        initial_prompt:
          promptToSend === "" && attachments().length === 0
            ? null
            : { text: promptToSend, attachments: [...attachments()] },
        initial_config: initialConfig(),
        mode: isProvisionerMode() || sessionMode() === "default" ? null : sessionMode(),
        extra_args: extraArgs,
        fork: forkSessionId()
          ? {
              from_session_id: forkSessionId(),
              up_to_node_seq: forkUpToNodeSeq(),
            }
          : null,
      });

      toast.success(
        useProvisioner
          ? m.new_session_ephemeral_started()
          : forkSessionId()
            ? m.new_session_fork_started()
            : m.new_session_started(),
      );
      await sync.refreshSessions();
      setPromptText("");
      composer.clear();
      composerDrafts.clear(NEW_SESSION_DRAFT_ID);
      navigate(`/sessions/${response.session_id}`);
    } catch (error) {
      // The form state (including the prompt) is intentionally kept so the
      // user can retry without retyping.
      problems.report("create session", error);
      setBusy(false);
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (shouldSubmitOnEnter(event, prefs.sendShortcut)) {
      event.preventDefault();
      void handleStartSession();
    }
  }

  /** Save the launch settings for this project (best effort, local device only). */
  function persistPrefs(): void {
    const projectId = selectedProject()?.project_id ?? "";
    // Do not remember ephemeral VMs because agent availability differs per node
    if (projectId === "" || isProvisionerMode()) return;
    saveNewSessionPrefs(projectId, {
      agent: effectiveAgent(),
      mode: sessionMode(),
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
    setChosenProjectId(projectId);
    // Everything else is project-scoped: re-resolve node, path and memory.
    setChosenNodeId(null);
    setDeepLinkNodeActive(false);
    setChosenPath("");
    setChosenAgentId("");
    setChosenMode(null);
    setManualAgentChoice(false);
    setUseCustomPath(false);
    setProjectOpen(false);
  }

  function selectNode(id: string): void {
    setRunTarget("node");
    setChosenNodeId(id);
    setChosenPath("");
    setUseCustomPath(false);
    setEnvironmentOpen(false);
  }

  function selectProvisioner(name: string): void {
    setRunTarget("provisioner");
    setProvisionerName(name);
    // Keep the popover open: it also hosts the clone branch and connectivity test.
  }

  function selectAgent(id: string): void {
    // Mode IDs are agent-specific: a selection for the previous agent no longer applies.
    if (id !== effectiveAgent()) setChosenMode("default");
    setChosenAgentId(id);
    setManualAgentChoice(false);
    setAgentOpen(false);
    persistPrefs();
  }

  function onManualAgentInput(): void {
    setManualAgentChoice(true);
    // The ID being typed is a different agent until proven otherwise.
    setChosenMode("default");
    persistPrefs();
  }

  /** Switch between the catalog list and the free-form agent ID input. */
  function toggleManualAgent(): void {
    setManualAgentChoice(!manualAgent());
    // Start from the agent currently in effect instead of an empty field.
    if (!manualAgent() && chosenAgentId() === "") setChosenAgentId(effectiveAgent());
  }

  function selectPath(path: string): void {
    setChosenPath(path);
    setUseCustomPath(false);
    setDirectoryOpen(false);
  }

  function resetPathToDefault(): void {
    setChosenPath("");
    setUseCustomPath(false);
  }

  function openFolderBrowser(): void {
    setDirectoryOpen(false);
    setBrowserOpen(true);
  }

  function openFileBrowser(): void {
    setFileBrowserOpen(true);
  }

  function onFileInputChange(event: Event & { currentTarget: HTMLInputElement }): void {
    const files = event.currentTarget.files;
    if (files) {
      void composer.addImageFiles(files);
      event.currentTarget.value = "";
    }
  }

  const projectIcon = () => <IconFolderGit2 class="text-muted-foreground size-3.5" />;
  const directoryIcon = () => <IconGitBranch class="text-muted-foreground size-3.5" />;
  const environmentIcon = () => (
    <Show
      when={isProvisionerMode()}
      fallback={
        <span class="relative inline-flex">
          <IconServer class="text-muted-foreground size-3.5" />
          <span
            class={`ring-background absolute -right-1 -bottom-0.5 size-1.5 rounded-full ring-1 ${nodeAvailability().dotColorClass}`}
          />
        </span>
      }
    >
      <IconBox class="text-muted-foreground size-3.5" />
    </Show>
  );
  const agentIcon = () => <AgentIcon icon={selectedAgent()?.icon ?? null} class="size-3.5" />;
  const modelIcon = () => <IconSparkles class="text-muted-foreground size-3.5" />;

  return (
    <>
      <div class="mx-auto flex min-h-full w-full max-w-[680px] flex-col justify-center px-4 py-8 md:py-12">
        <div class="flex flex-col gap-5">
          <div class="flex flex-col items-center gap-2.5 text-center">
            <div class="bg-signal-soft text-signal flex size-11 items-center justify-center rounded-xl">
              <IconSparkles class="size-5" />
            </div>
            <h1 class="text-2xl font-semibold tracking-tight">{m.new_session_title()}</h1>
            <p class="text-muted-foreground max-w-md text-[13px]">{m.new_session_description()}</p>
          </div>

          <Show when={forkSessionId()}>
            {(forkId) => (
              <div class="bg-signal-soft/70 border-signal/25 flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
                <div class="flex min-w-0 items-center gap-2">
                  <IconGitFork class="text-primary size-4 shrink-0" />
                  <span class="truncate">
                    <code class="bg-background/50 rounded px-1 py-0.5 font-mono">
                      {forkId().slice(0, 8)}
                    </code>
                    — {m.new_session_fork_banner_description()}
                  </span>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-6 shrink-0 px-2 text-[13px]"
                  onClick={() => {
                    setForkSessionId("");
                    setForkUpToNodeSeq(null);
                  }}
                >
                  <IconX class="mr-1 size-3.5" />
                  {m.new_session_fork_clear()}
                </Button>
              </div>
            )}
          </Show>

          <Show when={sync.projectsLoading && projects().length === 0}>
            <div class="bg-chip text-muted-foreground border-border flex items-center justify-center gap-2 rounded-lg border p-3 text-[13px]">
              <IconLoaderCircle class="size-3.5 animate-spin" />
              {m.common_loading()}
            </div>
          </Show>
          <Show when={!sync.projectsLoading && projects().length === 0 && sync.projectsError === null}>
            <div class="border-warn/35 bg-warn-soft/60 flex flex-wrap items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
              <span class="flex items-center gap-2">
                <IconFolderGit2 class="size-4 shrink-0" />
                {m.new_session_no_projects()}
              </span>
              <a
                href="/projects"
                class="bg-background hover:bg-muted rounded-md border px-2.5 py-1 text-[13px] font-medium"
              >
                {m.new_session_register_project()}
              </a>
            </div>
          </Show>
          <Show when={sync.projectsError !== null}>
            <div class="border-danger/30 bg-danger-soft/60 text-danger flex items-center justify-between gap-2 rounded-lg border p-3 text-[13px]">
              <span class="truncate">{sync.projectsError}</span>
              <Button variant="ghost" size="xs" onClick={() => void sync.refreshProjects()}>
                {m.common_retry()}
              </Button>
            </div>
          </Show>

          <ComposerCard
            above={
              <div class="flex flex-wrap items-center gap-1.5 px-1">
                <PickerPill
                  label={m.new_session_project()}
                  value={projectLabel()}
                  icon={projectIcon()}
                  open={projectOpen()}
                  onopenchange={setProjectOpen}
                  disabled={projects().length === 0}
                >
                  <Show
                    when={projects().length > 0}
                    fallback={
                      <div class="p-1">
                        <a
                          href="/projects"
                          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
                        >
                          {m.new_session_register_project()}
                        </a>
                      </div>
                    }
                  >
                    <div class="max-h-72 overflow-y-auto p-1">
                      <For each={projects()}>
                        {(project) => (
                          <PickerItem
                            selected={project.project_id === selectedProject()?.project_id}
                            onselect={() => selectProject(project.project_id)}
                          >
                            <span class="block truncate">{project.name}</span>
                          </PickerItem>
                        )}
                      </For>
                    </div>
                    <div class="border-t p-1">
                      <a
                        href="/projects"
                        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
                      >
                        {m.new_session_manage_projects()}
                      </a>
                    </div>
                  </Show>
                </PickerPill>

                <PickerPill
                  label={m.new_session_working_directory()}
                  value={directoryLabel()}
                  icon={directoryIcon()}
                  contentClass="w-[min(24rem,calc(100vw-1.5rem))]"
                  open={directoryOpen()}
                  onopenchange={setDirectoryOpen}
                  disabled={isProvisionerMode() || selectedProject() === null}
                >
                  <Show when={isProvisionerMode()}>
                    <p class="text-muted-foreground p-2 text-[13px]">{m.new_session_ephemeral_note()}</p>
                  </Show>
                  <Show when={!isProvisionerMode() && pathMode() === "new_worktree"}>
                    <div class="flex flex-col gap-2 p-2">
                      <div class="flex flex-col gap-1">
                        <span class="text-muted-foreground text-[11px]">
                          {m.new_session_new_worktree()}
                        </span>
                        <Input
                          value={branch()}
                          onValueChange={setBranch}
                          placeholder="feat/new-task"
                          class="h-8 text-[13px]"
                        />
                      </div>
                      <div class="flex flex-col gap-1">
                        <span class="text-muted-foreground text-[11px]">
                          {m.new_session_base_branch()}
                        </span>
                        <Input
                          value={baseBranch()}
                          onValueChange={setBaseBranch}
                          placeholder="main"
                          class="h-8 text-[13px]"
                        />
                      </div>
                      <p class="text-muted-foreground text-[11px]">
                        {m.new_session_worktree_description()}
                      </p>
                      <button
                        type="button"
                        class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
                        onClick={() => {
                          setPathMode("existing");
                          setDirectoryOpen(false);
                        }}
                      >
                        {m.new_session_back_to_list()}
                      </button>
                    </div>
                  </Show>
                  <Show when={!isProvisionerMode() && pathMode() === "existing" && useCustomPath()}>
                    <div class="flex flex-col gap-2 p-2">
                      <Input
                        value={chosenPath()}
                        onValueChange={setChosenPath}
                        placeholder={m.new_session_path_placeholder()}
                        class="h-8 font-mono text-[13px]"
                        autofocus
                      />
                      <div class="flex flex-wrap items-center gap-1.5">
                        <Button variant="outline" size="sm" onClick={openFolderBrowser}>
                          {m.new_session_browse()}
                        </Button>
                        <Show when={pathCandidates().length > 0}>
                          <Button variant="ghost" size="sm" onClick={resetPathToDefault}>
                            {m.new_session_back_to_list()}
                          </Button>
                        </Show>
                      </div>
                      <Show when={localPath().trim() !== "" && selectedCandidate() === null}>
                        <p class="text-muted-foreground text-[11px]">
                          {m.new_session_unregistered_path()}
                        </p>
                      </Show>
                      <Show when={selectedCandidate() !== null && !selectedCandidate()?.exists}>
                        <p class="text-destructive text-[11px]">{m.new_session_path_not_exists()}</p>
                      </Show>
                    </div>
                  </Show>
                  <Show
                    when={
                      !isProvisionerMode() &&
                      pathMode() === "existing" &&
                      !useCustomPath() &&
                      pathCandidates().length > 0
                    }
                  >
                    <div class="max-h-72 overflow-y-auto p-1">
                      <For each={pathCandidates()}>
                        {(candidate) => (
                          <PickerItem
                            selected={candidate.path === localPath()}
                            onselect={() => selectPath(candidate.path)}
                          >
                            <span class="block truncate">{pathBasename(candidate.path)}</span>
                            <span
                              class={`text-muted-foreground block truncate text-[11px] ${candidate.exists ? "" : "text-destructive"}`}
                              title={candidateLabel(candidate)}
                            >
                              {candidate.path} · {candidateDetail(candidate)}
                            </span>
                          </PickerItem>
                        )}
                      </For>
                    </div>
                    <div class="flex flex-wrap items-center gap-1 border-t p-1">
                      <button
                        type="button"
                        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
                        onClick={openFolderBrowser}
                      >
                        {m.new_session_browse()}
                      </button>
                      <button
                        type="button"
                        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
                        onClick={() => {
                          if (chosenPath() === "") setChosenPath(localPath());
                          setUseCustomPath(true);
                        }}
                      >
                        {m.new_session_enter_path()}
                      </button>
                      <button
                        type="button"
                        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground rounded-md px-2 py-1.5 text-[11px]"
                        onClick={() => {
                          setPathMode("new_worktree");
                          setDirectoryOpen(false);
                        }}
                      >
                        {m.new_session_new_worktree()}
                      </button>
                    </div>
                  </Show>
                  <Show
                    when={
                      !isProvisionerMode() &&
                      pathMode() === "existing" &&
                      !useCustomPath() &&
                      pathCandidates().length === 0
                    }
                  >
                    <div class="flex flex-col gap-2 p-2">
                      <p class="text-muted-foreground text-[13px]">
                        {m.new_session_no_candidate({
                          project: selectedProject()?.name ?? m.new_session_this_project(),
                        })}
                      </p>
                      <div class="flex flex-wrap items-center gap-1.5">
                        <Button variant="outline" size="sm" onClick={openFolderBrowser}>
                          {m.new_session_select_folder()}
                        </Button>
                        <a
                          href="/projects"
                          class="text-muted-foreground hover:text-foreground text-[11px] underline underline-offset-2"
                        >
                          {m.new_session_go_to_project_settings()}
                        </a>
                      </div>
                      <For each={otherNodesWithCandidates()}>
                        {(otherNode) => (
                          <button
                            type="button"
                            class="text-muted-foreground hover:text-foreground self-start text-[11px] underline"
                            onClick={() => selectNode(otherNode.node_id)}
                          >
                            {m.new_session_registered_on_node({ node: otherNode.name })}
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
                </PickerPill>

                <span class="flex-1" />

                <PickerPill
                  label={m.new_session_environment()}
                  value={environmentLabel()}
                  icon={environmentIcon()}
                  contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
                  open={environmentOpen()}
                  onopenchange={setEnvironmentOpen}
                  disabled={nodes().length === 0 && provisioners().length === 0}
                >
                  <div class="max-h-80 overflow-y-auto p-1">
                    <div class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase">
                      {m.new_session_node_section()}
                    </div>
                    <Show
                      when={onlineNodes().length === 0}
                      fallback={
                        <For each={nodes()}>
                          {(node) => {
                            const availability = () => getNodeAvailability(node, sync.wsConnected);
                            return (
                              <PickerItem
                                selected={!isProvisionerMode() && node.node_id === nodeId()}
                                disabled={!availability().isAvailable}
                                onselect={() => selectNode(node.node_id)}
                              >
                                <span class="flex items-center gap-1.5">
                                  <span
                                    class={`size-1.5 shrink-0 rounded-full ${availability().dotColorClass}`}
                                  />
                                  <span class="truncate">{node.name}</span>
                                </span>
                                <span class="text-muted-foreground block truncate text-[11px]">
                                  {availability().statusText}
                                </span>
                              </PickerItem>
                            );
                          }}
                        </For>
                      }
                    >
                      <p class="text-muted-foreground px-2 py-1.5 text-[13px]">
                        {nodes().length === 0
                          ? m.new_session_no_nodes()
                          : m.new_session_no_online_nodes()}
                      </p>
                    </Show>

                    <div class="border-t p-1 pt-2">
                      <div class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase">
                        {m.new_session_ephemeral_section()}
                      </div>
                      <Show
                        when={provisioners().length > 0}
                        fallback={
                          <Show
                            when={provisionerLoadError() !== null}
                            fallback={
                              <>
                                <p class="text-muted-foreground px-2 py-1 text-[11px]">
                                  {m.new_session_provisioner_undefined()}
                                </p>
                                <p class="text-muted-foreground px-2 pb-1 text-[11px]">
                                  {m.new_session_provisioner_config_hint()}
                                </p>
                              </>
                            }
                          >
                            <div class="flex items-center gap-1.5 px-2 py-1 text-[11px]">
                              <span class="text-destructive truncate">{m.common_load_failed()}</span>
                              <button
                                type="button"
                                class="text-muted-foreground hover:text-foreground underline"
                                onClick={() => void loadProvisioners()}
                              >
                                {m.common_retry()}
                              </button>
                            </div>
                          </Show>
                        }
                      >
                        <For each={provisioners()}>
                          {(provisioner) => (
                            <PickerItem
                              selected={
                                isProvisionerMode() && provisioner.name === provisionerName()
                              }
                              onselect={() => selectProvisioner(provisioner.name)}
                            >
                              <span class="block truncate">
                                {provisioner.description ?? provisioner.name}
                              </span>
                              <span class="text-muted-foreground block truncate text-[11px]">
                                {provisioner.name}
                              </span>
                            </PickerItem>
                          )}
                        </For>
                      </Show>

                      <Show when={isProvisionerMode() && selectedProvisioner() !== null}>
                        <div class="mt-2 flex flex-col gap-2 border-t px-2 pt-2">
                          <div class="flex flex-col gap-1">
                            <span class="text-muted-foreground text-[11px]">
                              {m.new_session_clone_branch()}
                            </span>
                            <Input
                              value={branch()}
                              onValueChange={setBranch}
                              placeholder={m.new_session_clone_branch_placeholder()}
                              class="h-8 text-[13px]"
                            />
                          </div>
                          <div class="flex items-center justify-between gap-2">
                            <span class="text-muted-foreground min-w-0 truncate text-[11px]">
                              {selectedProvisioner()?.name ?? ""}
                            </span>
                            <Button
                              variant="outline"
                              size="sm"
                              class="h-7 shrink-0 px-2 text-[13px]"
                              disabled={testBusy()}
                              title={m.new_session_provisioner_test_button_title()}
                              onClick={() => void runProvisionerTest(provisionerName())}
                            >
                              {m.new_session_test()}
                            </Button>
                          </div>
                        </div>
                      </Show>
                    </div>
                  </div>
                </PickerPill>

                <Show when={attachments().length > 0}>
                  <div class="flex w-full flex-wrap gap-2 pt-1">
                    <For each={attachments()}>
                      {(attachment, index) => (
                        <div class="bg-chip border-border flex items-center gap-1.5 rounded-md border px-2 py-1 text-[13px]">
                          <Show
                            when={attachment.data_b64}
                            fallback={<IconFile class="text-muted-foreground size-3.5" />}
                          >
                            <img
                              src={`data:${attachment.mime_type};base64,${attachment.data_b64 ?? ""}`}
                              alt={attachment.file_name}
                              class="size-8 rounded object-cover"
                            />
                          </Show>
                          <span class="max-w-40 truncate">{attachment.file_name}</span>
                          <button
                            type="button"
                            class="text-muted-foreground hover:text-foreground"
                            title={m.chat_attachment_remove()}
                            onClick={() => composer.remove(index())}
                          >
                            <IconX class="size-3.5" />
                          </button>
                        </div>
                      )}
                    </For>
                  </div>
                </Show>
              </div>
            }
            selectors={
              <>
                <PickerPill
                  label={m.new_session_agent()}
                  value={agentLabel()}
                  icon={agentIcon()}
                  class="shrink-0"
                  contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
                  open={agentOpen()}
                  onopenchange={setAgentOpen}
                  disabled={forkSessionId() !== ""}
                >
                  <Show
                    when={manualAgent()}
                    fallback={
                      <Show
                        when={catalogLoading()}
                        fallback={
                          <Show
                            when={catalogError() !== null}
                            fallback={
                              <Show
                                when={availableAgents().length === 0}
                                fallback={
                                  <div class="max-h-72 overflow-y-auto p-1">
                                    <For each={availableAgents()}>
                                      {(agent) => (
                                        <PickerItem
                                          selected={agent.id === effectiveAgent()}
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
                                      )}
                                    </For>
                                  </div>
                                }
                              >
                                <p class="text-muted-foreground p-2 text-[13px]">
                                  {m.new_session_no_agents()}
                                </p>
                              </Show>
                            }
                          >
                            <div class="flex flex-col gap-2 p-2">
                              <p class="text-destructive text-[11px] break-words">
                                {catalogError()}
                              </p>
                              <Button
                                variant="outline"
                                size="sm"
                                class="self-start"
                                onClick={() => void loadAgentCatalog(nodeId())}
                              >
                                {m.common_retry()}
                              </Button>
                            </div>
                          </Show>
                        }
                      >
                        <p class="text-muted-foreground flex items-center gap-1.5 px-2 py-2 text-[13px]">
                          <IconLoaderCircle class="size-3.5 animate-spin" />
                          {m.common_loading()}
                        </p>
                      </Show>
                    }
                  >
                    <div class="flex flex-col gap-2 p-2">
                      <Input
                        value={chosenAgentId()}
                        onValueChange={setChosenAgentId}
                        onInput={onManualAgentInput}
                        placeholder={m.new_session_agent_id_placeholder()}
                        class="h-8 font-mono text-[13px]"
                        disabled={forkSessionId() !== ""}
                      />
                      <p class="text-muted-foreground text-[11px]">{m.new_session_agent_choose_hint()}</p>
                    </div>
                  </Show>
                  <div class="border-t p-1">
                    <button
                      type="button"
                      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block w-full rounded-md px-2 py-1.5 text-left text-[13px] disabled:pointer-events-none disabled:opacity-50"
                      disabled={forkSessionId() !== ""}
                      onClick={toggleManualAgent}
                    >
                      {manualAgent() ? m.new_session_choose_from_list() : m.new_session_enter_id()}
                    </button>
                    <a
                      href={agentsSettingsHref()}
                      class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
                    >
                      {m.new_session_manage_agents()}
                    </a>
                  </div>
                </PickerPill>

                <Show when={!isProvisionerMode()}>
                  <Show when={probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())}>
                    <ModePicker modes={[]} loading={true} class="shrink-0" onchange={() => {}} />
                  </Show>
                  <Show
                    when={
                      !(probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())) &&
                      availableModes() !== null &&
                      (availableModes()?.length ?? 0) > 0
                    }
                  >
                    <ModePicker
                      modes={availableModes() ?? []}
                      value={sessionMode() === "default" ? null : sessionMode()}
                      allowDefault={true}
                      class="shrink-0"
                      onchange={(modeId) => {
                        setChosenMode(modeId ?? "default");
                        persistPrefs();
                      }}
                    />
                  </Show>
                </Show>

                <Show when={isProvisionerMode()}>
                  <PickerPill
                    label={m.new_session_model()}
                    value={m.new_session_models_after_start()}
                    icon={modelIcon()}
                    class="shrink-0"
                    disabled={true}
                  />
                </Show>
                <Show when={!isProvisionerMode() && probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())}>
                  <PickerPill
                    label={m.new_session_model()}
                    value={m.new_session_models_loading()}
                    icon={modelIcon()}
                    class="shrink-0"
                    loading={true}
                    disabled={true}
                  />
                </Show>
                <Show
                  when={
                    !isProvisionerMode() &&
                    !(probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())) &&
                    configOptions().length > 0
                  }
                >
                  <For each={configOptions()}>
                    {(option) => (
                      <ConfigOptionPicker
                        option={option}
                        value={selectedConfigValues()[option.key]}
                        class="shrink-0"
                        favoriteScope={effectiveAgent()}
                        onchange={(value) => changeConfigOption(option.key, value)}
                        showRefresh={isModelOption(option)}
                        onrefresh={() => void loadAgentCapabilities(true)}
                      />
                    )}
                  </For>
                </Show>
                <Show
                  when={
                    !isProvisionerMode() &&
                    !(probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())) &&
                    configOptions().length === 0 &&
                    capabilitiesError() !== null
                  }
                >
                  <PickerPill
                    label={m.new_session_model()}
                    value={m.new_session_model_default()}
                    icon={modelIcon()}
                    class="shrink-0"
                    contentClass="w-72 p-0"
                  >
                    <div class="flex flex-col gap-2 p-2 text-[13px]">
                      <p class="text-muted-foreground">{m.new_session_models_unavailable()}</p>
                      <p class="text-destructive text-[11px] break-words">{capabilitiesError()}</p>
                      <Button
                        variant="outline"
                        size="sm"
                        class="self-start"
                        onClick={() => void loadAgentCapabilities(true)}
                      >
                        {m.common_retry()}
                      </Button>
                    </div>
                  </PickerPill>
                </Show>
                <Show
                  when={
                    !isProvisionerMode() &&
                    !(probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())) &&
                    configOptions().length === 0 &&
                    capabilitiesError() === null &&
                    capabilities() !== null &&
                    capabilitiesKey() === probeKey()
                  }
                >
                  <PickerPill
                    label={m.new_session_model()}
                    value={m.new_session_model_default()}
                    icon={modelIcon()}
                    class="shrink-0"
                    contentClass="w-72 p-2"
                  >
                    <p class="text-muted-foreground text-[13px]">{m.new_session_models_empty()}</p>
                  </PickerPill>
                </Show>
                <Show
                  when={
                    !isProvisionerMode() &&
                    !(probeKey() !== "" && (capabilitiesLoading() || capabilitiesKey() !== probeKey())) &&
                    configOptions().length === 0 &&
                    capabilitiesError() === null &&
                    !(capabilities() !== null && capabilitiesKey() === probeKey())
                  }
                >
                  <PickerPill
                    label={m.new_session_model()}
                    value={m.new_session_model_default()}
                    icon={modelIcon()}
                    class="shrink-0"
                    disabled={true}
                  />
                </Show>
              </>
            }
            actions={
              <>
                <input
                  ref={setFileInput}
                  type="file"
                  accept="image/*"
                  multiple
                  class="hidden"
                  aria-label={m.chat_attach_image()}
                  onChange={onFileInputChange}
                />
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="rounded-full"
                  title={m.chat_attach_image()}
                  aria-label={m.chat_attach_image()}
                  onClick={() => fileInput()?.click()}
                >
                  <IconPaperclip class="size-4" />
                </Button>

                <ComposerMoreButton
                  label={m.composer_more_options()}
                  active={extraArgsText().trim() !== ""}
                  open={moreOpen()}
                  onopenchange={setMoreOpen}
                  contentClass="w-[min(20rem,calc(100vw-2rem))]"
                >
                  <div class="flex flex-col gap-2 p-2">
                    <Show when={!isProvisionerMode() && nodeId() !== ""}>
                      <button
                        type="button"
                        class="hover:bg-accent flex items-center gap-2 rounded-md px-1.5 py-1.5 text-left text-[13px]"
                        onClick={() => {
                          setMoreOpen(false);
                          openFileBrowser();
                        }}
                      >
                        <IconFile class="text-muted-foreground size-4" />
                        {m.chat_attach_file()}
                      </button>
                      <div class="border-t" />
                    </Show>
                    <div class="flex flex-col gap-1">
                      <Label for="new-session-extra-args" class="text-muted-foreground text-[11px]">
                        {m.new_session_extra_args()}
                      </Label>
                      <Input
                        id="new-session-extra-args"
                        value={extraArgsText()}
                        onValueChange={setExtraArgsText}
                        placeholder={m.new_session_extra_args_placeholder()}
                        class="h-8 font-mono text-[13px]"
                      />
                    </div>
                  </div>
                </ComposerMoreButton>

                <ComposerSendButton
                  busy={busy()}
                  disabled={!canSubmit() || busy()}
                  title={startTitle()}
                  label={m.new_session_start()}
                  onclick={() => void handleStartSession()}
                />
              </>
            }
          >
            <ComposerTextarea
              value={promptText()}
              onValueChange={setPromptText}
              onkeydown={onKeydown}
              onpaste={composer.onPaste}
              ondrop={composer.onDrop}
              rows={3}
              placeholder={m.new_session_prompt_placeholder()}
              ariaLabel={m.new_session_prompt_label()}
              disabled={busy()}
            />
          </ComposerCard>

          <div class="min-h-4 text-center text-[13px]">
            <Show when={busy()}>
              <span class="text-muted-foreground">{m.new_session_starting()}</span>
            </Show>
            <Show when={!busy() && blockingHint() !== ""}>
              <span class="text-destructive">{blockingHint()}</span>
            </Show>
            <Show
              when={
                !busy() &&
                blockingHint() === "" &&
                capabilitiesError() !== null &&
                capabilitiesKey() === probeKey()
              }
            >
              <span class="text-muted-foreground">{m.new_session_models_unavailable()}</span>
            </Show>
            <Show
              when={
                !busy() &&
                blockingHint() === "" &&
                !(capabilitiesError() !== null && capabilitiesKey() === probeKey()) &&
                canSubmit()
              }
            >
              <span class="text-muted-foreground hidden sm:inline">
                {prefs.sendShortcut === "enter"
                  ? m.new_session_enter_hint()
                  : m.new_session_enter_hint_ctrl()}
              </span>
              <span class="text-muted-foreground sm:hidden">{m.new_session_tap_hint()}</span>
            </Show>
          </div>
        </div>
      </div>

      <FolderBrowserDialog
        open={browserOpen()}
        onOpenChange={setBrowserOpen}
        nodeId={nodeId()}
        initialPath={localPath()}
        onSelect={(path) => {
          setChosenPath(path);
          setUseCustomPath(true);
        }}
      />

      <FolderBrowserDialog
        open={fileBrowserOpen()}
        onOpenChange={setFileBrowserOpen}
        nodeId={nodeId()}
        files={true}
        title={m.chat_attach_file()}
        onSelect={composer.addLocalFile}
      />

      <Dialog.Root open={testDialogOpen()} onOpenChange={setTestDialogOpen}>
        <Dialog.Content class="max-w-lg">
          <Dialog.Header>
            <Dialog.Title>
              {m.new_session_provisioner_test_title({ name: provisionerName() })}
            </Dialog.Title>
            <Dialog.Description>{m.new_session_provisioner_test_description()}</Dialog.Description>
          </Dialog.Header>
          <div class="flex flex-col gap-3 py-2 text-[13px]">
            <Show
              when={!testBusy()}
              fallback={
                <div class="text-muted-foreground flex flex-col items-center justify-center gap-2 py-8">
                  <IconLoaderCircle class="text-primary size-6 animate-spin" />
                  <span>{m.new_session_provisioner_starting()}</span>
                </div>
              }
            >
              <Show when={testResult()}>
                {(result) => (
                  <>
                    <div class="flex items-center gap-2 font-medium">
                      <Show
                        when={result().ok}
                        fallback={
                          <>
                            <IconXCircle class="text-destructive size-5" />
                            <span class="text-destructive">{m.new_session_test_failed()}</span>
                          </>
                        }
                      >
                        <IconCheckCircle2 class="text-ok size-5" />
                        <span class="text-ok">{m.new_session_test_success()}</span>
                        <Show when={result().node_id}>
                          {(nodeIdValue) => (
                            <span class="text-muted-foreground font-mono text-[11px]">
                              ({nodeIdValue()})
                            </span>
                          )}
                        </Show>
                      </Show>
                    </div>

                    <Show when={result().error}>
                      {(message) => (
                        <div class="bg-destructive/10 text-destructive border-destructive/20 rounded-md border p-2.5 font-mono text-[11px] whitespace-pre-wrap">
                          {message()}
                        </div>
                      )}
                    </Show>

                    <Show when={(result().log_lines?.length ?? 0) > 0}>
                      <div class="flex flex-col gap-1">
                        <span class="text-muted-foreground text-[11px]">
                          {m.new_session_run_log()}
                        </span>
                        <div class="bg-muted text-foreground max-h-48 overflow-y-auto rounded-md p-2 font-mono text-[11px] leading-tight whitespace-pre-wrap">
                          {result().log_lines.join("\n")}
                        </div>
                      </div>
                    </Show>
                  </>
                )}
              </Show>
            </Show>
          </div>
          <Dialog.Footer>
            <Button variant="outline" size="sm" onClick={() => setTestDialogOpen(false)}>
              {m.common_close()}
            </Button>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
}
