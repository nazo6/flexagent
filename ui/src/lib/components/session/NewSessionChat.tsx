import { createMemo, createSignal, For, Show, untrack } from "solid-js";
import { useNavigate, useSearchParams } from "@solidjs/router";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.tsx";
import ComposerCard from "#lib/components/composer/ComposerCard.tsx";
import ComposerMoreButton from "#lib/components/composer/ComposerMoreButton.tsx";
import ComposerSendButton from "#lib/components/composer/ComposerSendButton.tsx";
import ComposerTextarea from "#lib/components/composer/ComposerTextarea.tsx";
import ModePicker from "#lib/components/composer/ModePicker.tsx";
import { createComposerAttachments } from "#lib/composer-attachments.ts";
import { randomUuid } from "#lib/uuid.ts";
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
import { nodeSettingsPath, resolveDefaultNodeId } from "#lib/settings-routes.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { problems } from "#lib/stores/problems.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconCheckCircle2 from "~icons/lucide/check-circle-2";
import IconFile from "~icons/lucide/file";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconPaperclip from "~icons/lucide/paperclip";
import IconX from "~icons/lucide/x";
import IconXCircle from "~icons/lucide/x-circle";
import { createNewSessionTarget } from "./use-new-session-target.ts";
import { pathBasename } from "./new-session-candidates.ts";
import { createProvisionerTest } from "./provisioner-test.ts";
import { sessionModeFor, sessionPath } from "./new-session-mode.ts";
import EnvironmentPicker from "./EnvironmentPicker.tsx";
import { toCreateSessionRequest } from "./start-session-request.ts";
import AgentPicker from "./AgentPicker.tsx";
import DirectoryPicker from "./DirectoryPicker.tsx";
import ProjectPicker from "./ProjectPicker.tsx";
import ModelSelectors from "./ModelSelectors.tsx";
import { ForkBanner, NewSessionHeader, ProjectNotices } from "./NewSessionHeader.tsx";

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
   * Initial mode to request.
   *
   * A mode the agent does not offer is ignored instead of failing
   * `session/set_mode` (see `sessionModeFor`).
   */
  const sessionMode = createMemo((): SessionModePreference => {
    const projectId = selectedProject()?.project_id ?? "";
    // Remembered per project only; the probe tells whether the agent offers it.
    const remembered =
      projectId === "" ? "default" : (loadNewSessionPrefs(projectId)?.mode ?? "default");
    return sessionModeFor(chosenMode() ?? remembered, availableModes());
  });
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

  const composer = createComposerAttachments(draft.attachments, {
    onChange: (next) => composerDrafts.setAttachments(NEW_SESSION_DRAFT_ID, next),
  });
  const attachments = () => composer.attachments;

  /** Write the prompt through to the draft store (see `Composer.updateText`). */
  function updatePromptText(value: string): void {
    setPromptText(value);
    composerDrafts.setText(NEW_SESSION_DRAFT_ID, value);
  }

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

  // Provisioner connectivity test
  const provisionerTest = createProvisionerTest(() => setEnvironmentOpen(false));

  const target = createNewSessionTarget({
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
    initial: initialUrl,
    projects: () => sync.projects,
    nodes: () => sync.nodes,
    sessions: () => sync.sessions,
    wsConnected: () => sync.wsConnected,
    loadPrefs: loadNewSessionPrefs,
    loadConfig: loadSessionConfig,
    saveConfigOption: saveSessionConfigOption,
    fetchProvisioners: () => sync.connection.client.provisioners(),
    fetchAgents: (nodeId) => sync.connection.client.nodeAgents(nodeId),
    probeAgent: (nodeId, agentId, path, force, config) =>
      sync.connection.client.probeAgent(nodeId, agentId, path, force, config),
  });
  const {
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
    pathCandidates,
    localPath,
    selectedCandidate,
    otherNodesWithCandidates,
    capabilitiesLoading,
    capabilitiesError,
    probeTarget,
    probeReady,
    probePending,
    configOptions,
    availableModes,
    initialConfig,
  } = target;

  const projects = () => sync.projects;
  const nodes = () => sync.nodes;

  const agentsSettingsHref = createMemo(() =>
    nodeSettingsPath(
      "agents",
      (!isProvisionerMode() && nodeId() !== "" ? nodeId() : null) ??
        resolveDefaultNodeId(
          sync.connection.role === "hub",
          sync.nodes,
          sync.connection.systemInfo?.node_id,
        ),
    ),
  );

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

  async function handleStartSession(initialPrompt?: string): Promise<void> {
    const project = selectedProject();
    if (!canSubmit() || project === null) return;
    setBusy(true);
    const promptToSend = (initialPrompt ?? promptText()).trim();
    const useProvisioner = isProvisionerMode() && selectedProvisioner() !== null;
    const extraArgs = extraArgsText().trim() ? extraArgsText().trim().split(/\s+/) : null;
    // Remember on the local device for the next new session
    persistPrefs({ agent: effectiveAgent(), mode: sessionMode() });

    try {
      const response = await sync.connection.client.createSession(
        toCreateSessionRequest({
          commandId: randomUuid(),
          projectId: project.project_id,
          provisioner: useProvisioner ? (selectedProvisioner()?.name ?? null) : null,
          nodeId: nodeId(),
          localPath: localPath(),
          worktree:
            pathMode() === "new_worktree" || (useProvisioner && branch().trim() !== "")
              ? {
                  branch: branch().trim(),
                  baseBranch: useProvisioner ? "" : baseBranch(),
                }
              : null,
          agentId: effectiveAgent(),
          prompt: promptToSend,
          attachments: attachments(),
          initialConfig: initialConfig(),
          mode: sessionMode(),
          extraArgs,
          fork: forkSessionId()
            ? { sessionId: forkSessionId(), upToNodeSeq: forkUpToNodeSeq() }
            : null,
        }),
      );

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
      navigate(sessionPath(response.session_id));
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
  function persistPrefs(selection: { agent: string; mode: SessionModePreference }): void {
    const projectId = selectedProject()?.project_id ?? "";
    // Do not remember ephemeral VMs because agent availability differs per node
    if (projectId === "" || isProvisionerMode()) return;
    saveNewSessionPrefs(projectId, selection);
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
    const previous = effectiveAgent();
    const changed = id !== previous;
    const nextMode = changed ? "default" : sessionMode();
    if (changed) setChosenMode(nextMode);
    setChosenAgentId(id);
    setManualAgentChoice(false);
    setAgentOpen(false);
    persistPrefs({ agent: id, mode: nextMode });
  }

  function onManualAgentInput(event: InputEvent & { currentTarget: HTMLInputElement }): void {
    const agent = event.currentTarget.value;
    setManualAgentChoice(true);
    // The ID being typed is a different agent until proven otherwise.
    setChosenMode("default");
    persistPrefs({ agent, mode: "default" });
  }

  /** Switch between the catalog list and the free-form agent ID input. */
  function toggleManualAgent(): void {
    const next = !manualAgent();
    setManualAgentChoice(next);
    // Start from the agent currently in effect instead of an empty field.
    if (next && chosenAgentId() === "") setChosenAgentId(effectiveAgent());
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

  return (
    <>
      <div class="mx-auto flex min-h-full w-full max-w-[680px] flex-col justify-center px-4 py-8 md:py-12">
        <div class="flex flex-col gap-5">
          <NewSessionHeader />

          <Show when={forkSessionId()}>
            {(forkId) => (
              <ForkBanner
                sessionId={forkId()}
                onClear={() => {
                  setForkSessionId("");
                  setForkUpToNodeSeq(null);
                }}
              />
            )}
          </Show>

          <ProjectNotices
            loading={sync.projectsLoading}
            isEmpty={projects().length === 0}
            error={sync.projectsError}
            onRetry={() => void sync.refreshProjects()}
          />

          <ComposerCard
            above={
              <div class="flex flex-wrap items-center gap-1.5 px-1">
                <ProjectPicker
                  label={projectLabel()}
                  open={projectOpen()}
                  onOpenChange={setProjectOpen}
                  disabled={projects().length === 0}
                  projects={projects()}
                  selectedProjectId={selectedProject()?.project_id ?? null}
                  onSelect={selectProject}
                />

                <DirectoryPicker
                  label={directoryLabel()}
                  open={directoryOpen()}
                  onOpenChange={setDirectoryOpen}
                  provisionerMode={isProvisionerMode()}
                  disabled={selectedProject() === null}
                  pathMode={pathMode()}
                  onPathModeChange={setPathMode}
                  branch={branch()}
                  onBranchChange={setBranch}
                  baseBranch={baseBranch()}
                  onBaseBranchChange={setBaseBranch}
                  customPath={useCustomPath()}
                  typedPath={chosenPath()}
                  onTypedPathChange={setChosenPath}
                  localPath={localPath()}
                  selectedCandidate={selectedCandidate()}
                  candidates={pathCandidates()}
                  onSelectCandidate={selectPath}
                  onResetToDefault={resetPathToDefault}
                  onBrowse={openFolderBrowser}
                  onEnterPath={() => {
                    if (chosenPath() === "") setChosenPath(localPath());
                    setUseCustomPath(true);
                  }}
                  projectName={selectedProject()?.name ?? ""}
                  otherNodes={otherNodesWithCandidates()}
                  onSelectNode={selectNode}
                />

                <span class="flex-1" />

                <EnvironmentPicker
                  label={environmentLabel()}
                  open={environmentOpen()}
                  onOpenChange={setEnvironmentOpen}
                  provisionerMode={isProvisionerMode()}
                  nodes={nodes()}
                  onlineNodeCount={onlineNodes().length}
                  selectedNodeId={nodeId()}
                  onSelectNode={selectNode}
                  provisioners={provisioners()}
                  provisionerName={provisionerName()}
                  provisionerLoadError={provisionerLoadError()}
                  onRetryProvisioners={() => void target.loadProvisioners()}
                  onSelectProvisioner={selectProvisioner}
                  branch={branch()}
                  onBranchChange={setBranch}
                  selectedProvisioner={selectedProvisioner()}
                  testBusy={provisionerTest.busy()}
                  onTest={() => void provisionerTest.run(provisionerName())}
                  nodeAvailabilityDotClass={nodeAvailability().dotColorClass}
                />

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
                <AgentPicker
                  label={agentLabel()}
                  selectedIcon={selectedAgent()?.icon ?? null}
                  open={agentOpen()}
                  onOpenChange={setAgentOpen}
                  locked={forkSessionId() !== ""}
                  manual={manualAgent()}
                  agentId={chosenAgentId()}
                  onAgentIdChange={setChosenAgentId}
                  onManualInput={onManualAgentInput}
                  catalogLoading={catalogLoading()}
                  catalogError={catalogError()}
                  agents={availableAgents()}
                  effectiveAgentId={effectiveAgent()}
                  onSelect={selectAgent}
                  onRetry={() => void target.reloadAgentCatalog()}
                  onToggleManual={toggleManualAgent}
                  agentsSettingsHref={agentsSettingsHref()}
                />

                <Show when={!isProvisionerMode()}>
                  <Show when={probeTarget() && probePending()}>
                    <ModePicker modes={[]} loading={true} class="shrink-0" onChange={() => {}} />
                  </Show>
                  <Show
                    when={
                      !(probeTarget() && probePending()) &&
                      availableModes() !== null &&
                      (availableModes()?.length ?? 0) > 0
                    }
                  >
                    <ModePicker
                      modes={availableModes() ?? []}
                      value={sessionMode() === "default" ? null : sessionMode()}
                      allowDefault={true}
                      class="shrink-0"
                      onChange={(modeId) => {
                        const mode = modeId ?? "default";
                        setChosenMode(mode);
                        persistPrefs({ agent: effectiveAgent(), mode });
                      }}
                    />
                  </Show>
                </Show>

                <ModelSelectors
                  fork={isProvisionerMode()}
                  provisionerMode={isProvisionerMode()}
                  probeReady={probeReady()}
                  loading={capabilitiesLoading()}
                  capabilitiesReady={target.capabilitiesReady()}
                  capabilitiesError={capabilitiesError()}
                  options={configOptions()}
                  values={target.selectedConfigValues()}
                  favoriteScope={effectiveAgent()}
                  onChange={(key, value) => target.changeConfigOption(key, value)}
                  onRefresh={() => void target.refreshCapabilities()}
                />
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
                  onOpenChange={setMoreOpen}
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
                  onClick={() => void handleStartSession()}
                />
              </>
            }
          >
            <ComposerTextarea
              value={promptText()}
              onValueChange={updatePromptText}
              onKeyDown={onKeydown}
              onPaste={composer.onPaste}
              onDrop={composer.onDrop}
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
                !busy() && blockingHint() === "" && capabilitiesError() !== null && probeReady()
              }
            >
              <span class="text-muted-foreground">{m.new_session_models_unavailable()}</span>
            </Show>
            <Show
              when={
                !busy() &&
                blockingHint() === "" &&
                !(capabilitiesError() !== null && probeReady()) &&
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

      <Dialog.Root open={provisionerTest.dialogOpen()} onOpenChange={provisionerTest.setDialogOpen}>
        <Dialog.Content class="max-w-lg">
          <Dialog.Header>
            <Dialog.Title>
              {m.new_session_provisioner_test_title({ name: provisionerName() })}
            </Dialog.Title>
            <Dialog.Description>{m.new_session_provisioner_test_description()}</Dialog.Description>
          </Dialog.Header>
          <div class="flex flex-col gap-3 py-2 text-[13px]">
            <Show
              when={!provisionerTest.busy()}
              fallback={
                <div class="text-muted-foreground flex flex-col items-center justify-center gap-2 py-8">
                  <IconLoaderCircle class="text-primary size-6 animate-spin" />
                  <span>{m.new_session_provisioner_starting()}</span>
                </div>
              }
            >
              <Show when={provisionerTest.result()}>
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
            <Button
              variant="outline"
              size="sm"
              onClick={() => provisionerTest.setDialogOpen(false)}
            >
              {m.common_close()}
            </Button>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
}
