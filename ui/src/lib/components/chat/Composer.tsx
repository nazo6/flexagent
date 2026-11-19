import { createEffect, createMemo, createSignal, For, onSettled, Show, untrack } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
import ComposerCard from "#lib/components/composer/ComposerCard.tsx";
import ComposerSendButton from "#lib/components/composer/ComposerSendButton.tsx";
import ComposerTextarea from "#lib/components/composer/ComposerTextarea.tsx";
import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.tsx";
import ModePicker from "#lib/components/composer/ModePicker.tsx";
import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.tsx";
import { createComposerAttachments } from "#lib/composer-attachments.ts";
import { orderedConfigOptions } from "#lib/config-options.ts";
import { saveSessionConfigOption, shouldSubmitOnEnter } from "#lib/session-prefs.ts";
import { composerDrafts } from "#lib/stores/composer-drafts.ts";
import { prefs } from "#lib/stores/prefs.ts";
import { m } from "#lib/paraglide/messages.js";
import type { NodeSummary, SessionControlAction, SessionStatus } from "#lib/generated/bindings.ts";
import { sync } from "#lib/stores/app.ts";
import { problems } from "#lib/stores/problems.ts";
import type { SessionCapabilities } from "#lib/sync/reducer.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconAlertTriangle from "~icons/lucide/alert-triangle";
import IconBan from "~icons/lucide/ban";
import IconFile from "~icons/lucide/file";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconMinimize2 from "~icons/lucide/minimize-2";
import IconMoreHorizontal from "~icons/lucide/more-horizontal";
import IconPaperclip from "~icons/lucide/paperclip";
import IconShieldCheck from "~icons/lucide/shield-check";
import IconSlash from "~icons/lucide/slash";
import IconX from "~icons/lucide/x";

/** Imperative handle the session page uses to restore a reverted turn. */
export interface ComposerApi {
  restoreText(value: string): void;
}

export interface ComposerProps {
  sessionId: string;
  capabilities?: SessionCapabilities | null;
  node?: NodeSummary | null;
  status?: SessionStatus | null;
  permissionPolicy?: string | null;
  projectId?: string;
  /** Agent ID used to scope favorite models and remembered configuration. */
  agentId?: string;
  /**
   * The last send failed because native restore is unsupported
   * (`RESUME_REQUIRED`). Normally false (sending resumes a stopped session
   * automatically); only when true do we suggest resuming via history replay.
   */
  resumeRequired?: boolean;
  /** Counterpart of the Svelte `bind:resumeRequired`. */
  onResumeRequiredChange?: (value: boolean) => void;
  /** Receives the imperative handle once (`bind:this` + exported method in Svelte). */
  onReady?: (api: ComposerApi) => void;
}

/** Prompt composer for an open session. */
export default function Composer(props: ComposerProps) {
  // The session page keys this component by `sessionId`, so the initial value
  // is the right one for the whole lifetime of this instance.
  const draft = untrack(() => composerDrafts.get(props.sessionId));
  const [text, setText] = createSignal(draft.text);

  /** Replace the composer text (e.g. restoring a reverted user turn). */
  function restoreText(value: string): void {
    updateText(value);
  }
  // Hand the imperative handle to the page once this composer has settled:
  // writing the parent's state from the component body is outside Solid's
  // allowed write scopes.
  onSettled(() => props.onReady?.({ restoreText }));

  const [sending, setSending] = createSignal(false);
  const [controlBusy, setControlBusy] = createSignal(false);
  const [resuming, setResuming] = createSignal(false);
  const [fileBrowserOpen, setFileBrowserOpen] = createSignal(false);
  const [fileInput, setFileInput] = createSignal<HTMLInputElement>();
  /**
   * Policy choice made by this click, shown until the session projection
   * reports the new one. The derived value below drops it once the reported
   * policy changes, so no effect has to mirror the prop into local state.
   */
  const [optimisticPolicy, setOptimisticPolicy] = createSignal<{
    base: string | null;
    value: boolean;
  } | null>(null);

  const policy = () => props.permissionPolicy ?? null;
  const resumeRequired = () => props.resumeRequired ?? false;
  const setResumeRequired = (value: boolean): void => props.onResumeRequiredChange?.(value);

  const autoApprove = () => {
    const optimistic = optimisticPolicy();
    return optimistic !== null && optimistic.base === policy()
      ? optimistic.value
      : policy() === "auto_allow_all";
  };

  const composer = createComposerAttachments(draft.attachments, {
    onChange: (next) => composerDrafts.setAttachments(props.sessionId, next),
  });
  const attachments = () => composer.attachments;

  /**
   * Write the prompt through to the draft store.
   *
   * Every edit goes through here, so leaving the screen (or switching to
   * another session) does not discard the prompt without an effect that
   * mirrors the signal back into the store.
   */
  function updateText(value: string): void {
    setText(value);
    composerDrafts.setText(props.sessionId, value);
  }

  async function toggleAutoApprove(): Promise<void> {
    const next = autoApprove() ? "ask" : "auto_allow_all";
    setOptimisticPolicy({ base: policy(), value: next === "auto_allow_all" });
    try {
      const result = await sync.controlSession(props.sessionId, {
        action: "set_permission_policy",
        policy: next,
      });
      if (!result.success) {
        setOptimisticPolicy(null);
        if (result.error) problems.reportInfo("permission policy", result.error);
        return;
      }
      toast.info(autoApprove() ? m.chat_auto_approve_enabled() : m.chat_auto_approve_disabled());
    } catch (error) {
      // Never leave the optimistic toggle showing a policy the server rejected.
      setOptimisticPolicy(null);
      problems.report("permission policy", error);
    }
  }

  const isRunning = () =>
    props.status === "running" ||
    props.status === "waiting_permission" ||
    props.status === "waiting_input";
  const isStopped = () => props.status === "stopped" || props.status === "error";

  // Close the resume suggestion once the session becomes active
  createEffect(
    () => props.status ?? null,
    (status) => {
      if (
        status === "idle" ||
        status === "running" ||
        status === "waiting_permission" ||
        status === "waiting_input"
      ) {
        setResumeRequired(false);
      }
    },
  );

  const nodeAvail = () => getNodeAvailability(props.node ?? null, sync.wsConnected);

  /** Agent config options (model / thought level / agent-specific), newest first. */
  const configOptions = createMemo(() =>
    orderedConfigOptions(props.capabilities?.configOptions ?? []),
  );

  const slashQuery = () => {
    const value = text();
    return value.startsWith("/") && !value.includes(" ") ? value.slice(1).toLowerCase() : null;
  };

  const commandMatches = () => {
    const capabilities = props.capabilities ?? null;
    const query = slashQuery();
    if (capabilities === null || query === null) return [];
    return capabilities.availableCommands
      .filter((command) => command.name.toLowerCase().startsWith(query))
      .slice(0, 8);
  };

  /** Whether context compaction (`/compact`) is supported (detected via the synthetic entry). */
  const canCompact = () =>
    props.capabilities?.availableCommands.some((command) => command.name === "compact") ?? false;

  /** Keep the overflow menu visible only when it has at least one item. */
  const hasMoreActions = () => (props.node ?? null) !== null || (canCompact() && !isStopped());

  function applyCommand(name: string): void {
    updateText(`/${name} `);
  }

  const placeholderText = () => {
    const isEnter = prefs.sendShortcut === "enter";
    if (isStopped()) {
      return isEnter ? m.chat_placeholder_stopped_enter() : m.chat_placeholder_stopped();
    }
    if (isRunning()) {
      return isEnter ? m.chat_placeholder_running_enter() : m.chat_placeholder_running();
    }
    return isEnter ? m.chat_placeholder_idle_enter() : m.chat_placeholder_idle();
  };

  function onFileInputChange(event: Event & { currentTarget: HTMLInputElement }): void {
    const files = event.currentTarget.files;
    if (files) {
      void composer.addImageFiles(files);
      event.currentTarget.value = "";
    }
  }

  async function send(): Promise<void> {
    const value = text().trim();
    if ((value === "" && attachments().length === 0) || sending()) return;
    setSending(true);
    try {
      const result = await sync.sendPrompt(props.sessionId, {
        text: value,
        attachments: [...attachments()],
      });
      if (!result.success) {
        if (result.error?.code === "RESUME_REQUIRED") {
          // Automatic resume (native restore) is unsupported for stopped
          // sessions. Keep the input and suggest resuming via history replay.
          setResumeRequired(true);
        } else if (result.error) {
          problems.reportInfo("send prompt", result.error);
        } else {
          problems.warn("send prompt", m.chat_send_failed());
        }
        return;
      }
      updateText("");
      composer.clear();
      setResumeRequired(false);
    } catch (error) {
      // Keep the composer text so the user can retry without retyping.
      problems.report("send prompt", error);
    } finally {
      setSending(false);
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (shouldSubmitOnEnter(event, prefs.sendShortcut)) {
      event.preventDefault();
      void send();
    }
  }

  async function runControl(action: SessionControlAction): Promise<void> {
    if (controlBusy()) return;
    setControlBusy(true);
    try {
      const result = await sync.controlSession(props.sessionId, action);
      if (!result.success) {
        if (result.error) {
          problems.reportInfo("session control", result.error);
        } else {
          problems.warn("session control", m.chat_control_failed());
        }
      } else if (action.action === "cancel") {
        toast.info(m.chat_cancel_requested());
      } else if (
        action.action === "set_config" &&
        (props.projectId ?? "") !== "" &&
        (props.agentId ?? "") !== ""
      ) {
        saveSessionConfigOption(
          props.projectId ?? "",
          props.agentId ?? "",
          action.key,
          action.value,
        );
      }
    } catch (error) {
      problems.report("session control", error);
    } finally {
      setControlBusy(false);
    }
  }

  async function requestCompact(): Promise<void> {
    if (sending() || controlBusy()) return;
    try {
      const result = await sync.sendPrompt(props.sessionId, {
        text: "/compact",
        attachments: [],
      });
      if (!result.success) {
        if (result.error) {
          problems.reportInfo("compact context", result.error);
        } else {
          problems.warn("compact context", m.chat_compact_failed());
        }
      } else {
        toast.info(m.chat_compact_sent());
      }
    } catch (error) {
      problems.report("compact context", error);
    }
  }

  /** Resume a stopped session (including the history replay fallback). */
  async function resumeSession(): Promise<boolean> {
    if (sending() || resuming()) return false;
    setResuming(true);
    try {
      const result = await sync.resumeSession(props.sessionId);
      if (result.context_restored) {
        toast.success(m.chat_session_resumed_context());
      } else {
        toast.success(m.chat_session_resumed_replay());
      }
      return true;
    } catch (error) {
      problems.report("resume session", error);
      return false;
    } finally {
      setResuming(false);
    }
  }

  /** Resume a stopped session without native restore via history replay, then send the input. */
  async function resumeAndSend(): Promise<void> {
    if (sending() || (text().trim() === "" && attachments().length === 0)) return;
    if (!(await resumeSession())) return;
    setResumeRequired(false);
    await send();
  }

  return (
    <>
      <ComposerCard
        above={
          <div class="flex flex-col gap-2 px-1">
            <Show
              when={isStopped()}
              fallback={
                <Show when={(props.node ?? null) !== null && !nodeAvail().isAvailable}>
                  <div class="border-danger/25 bg-danger-soft/50 text-danger flex items-center gap-2 rounded-md border px-3 py-1.5 text-[13px]">
                    <IconAlertTriangle class="size-3.5 shrink-0" />
                    <span>
                      {m.chat_node_unavailable({
                        name: props.node?.name ?? "",
                        status: nodeAvail().statusText,
                      })}
                    </span>
                  </div>
                </Show>
              }
            >
              <div class="border-warn/25 bg-warn-soft/40 text-warn flex items-center gap-2 rounded-md border px-3 py-1 text-[13px]">
                <IconAlertTriangle class="size-3.5 shrink-0" />
                <span class="min-w-0 flex-1 truncate" title={m.chat_stopped_notice()}>
                  {m.chat_stopped_notice()}
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-6 shrink-0 text-[11px]"
                  disabled={sending() || resuming()}
                  title={m.chat_resume_title()}
                  onClick={() => void resumeSession()}
                >
                  <Show when={resuming()}>
                    <IconLoaderCircle class="size-3.5 animate-spin" />
                  </Show>
                  {resuming() ? m.chat_resuming() : m.chat_resume()}
                </Button>
              </div>
            </Show>

            <Show when={resumeRequired()}>
              <div class="border-warn/25 bg-warn-soft/40 text-warn flex flex-wrap items-center gap-2 rounded-md border px-3 py-1.5 text-[13px]">
                <IconAlertTriangle class="size-3.5 shrink-0" />
                <span>{m.chat_resume_required_notice()}</span>
                <Button
                  variant="outline"
                  size="sm"
                  class="ml-auto h-6 text-[11px]"
                  disabled={sending() || resuming()}
                  onClick={() => void resumeAndSend()}
                >
                  <Show when={resuming()}>
                    <IconLoaderCircle class="size-3.5 animate-spin" />
                  </Show>
                  {resuming() ? m.chat_resuming() : m.chat_resume_and_send()}
                </Button>
              </div>
            </Show>

            <Show when={commandMatches().length > 0}>
              <div class="flex flex-wrap gap-1">
                <For each={commandMatches()}>
                  {(command) => (
                    <button
                      type="button"
                      class="bg-chip hover:bg-chip-hover border-border flex items-center gap-1 rounded-md border px-2 py-1 text-[13px]"
                      onClick={() => applyCommand(command.name)}
                    >
                      <IconSlash class="size-3" />
                      <span class="font-mono">{command.name}</span>
                      <span class="text-muted-foreground">{command.description}</span>
                    </button>
                  )}
                </For>
              </div>
            </Show>

            <Show when={attachments().length > 0}>
              <div class="flex flex-wrap gap-2">
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
            <Show when={(props.capabilities?.availableModes.length ?? 0) > 0}>
              <ModePicker
                modes={props.capabilities?.availableModes ?? []}
                value={props.capabilities?.currentMode ?? null}
                class="shrink-0"
                onChange={(modeId) => {
                  if (modeId !== null) void runControl({ action: "set_mode", mode_id: modeId });
                }}
              />
            </Show>

            <For each={configOptions()} keyed={(option) => option.key}>
              {(option) => (
                <ConfigOptionPicker
                  option={option()}
                  class="shrink-0"
                  favoriteScope={props.agentId ?? ""}
                  onChange={(value) =>
                    void runControl({
                      action: "set_config",
                      key: option().key,
                      value,
                    })
                  }
                />
              )}
            </For>
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
            <Show when={!sync.wsConnected}>
              <span class="text-danger text-[11px]">{m.chat_connection_lost()}</span>
            </Show>
            <Button
              variant="outline"
              size="sm"
              class={`h-7 gap-1 rounded-full px-2 text-xs ${
                autoApprove()
                  ? "border-signal/40 bg-signal-soft text-signal hover:bg-signal-soft"
                  : ""
              }`}
              disabled={controlBusy() || sending()}
              title={m.chat_auto_approve_title()}
              aria-label={m.chat_auto_approve_title()}
              onClick={() => void toggleAutoApprove()}
            >
              <IconShieldCheck class="size-3.5" />
              <span class="hidden sm:inline">{m.chat_auto_approve()}</span>
            </Button>
            <Show when={hasMoreActions()}>
              <DropdownMenu.Root>
                <DropdownMenu.Trigger
                  class="text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/40 inline-flex size-7 shrink-0 items-center justify-center rounded-full outline-none transition-colors focus-visible:ring-2"
                  title={m.composer_more_options()}
                  aria-label={m.composer_more_options()}
                >
                  <IconMoreHorizontal class="size-4" />
                </DropdownMenu.Trigger>
                <DropdownMenu.Content align="end" class="w-52">
                  <Show when={props.node ?? null}>
                    <DropdownMenu.Item onSelect={() => setFileBrowserOpen(true)}>
                      <IconFile />
                      {m.chat_attach_file()}
                    </DropdownMenu.Item>
                  </Show>
                  <Show when={canCompact() && !isStopped()}>
                    <DropdownMenu.Item
                      disabled={controlBusy() || sending()}
                      title={m.chat_compact_title()}
                      onSelect={() => void requestCompact()}
                    >
                      <IconMinimize2 />
                      {m.chat_compact()}
                    </DropdownMenu.Item>
                  </Show>
                </DropdownMenu.Content>
              </DropdownMenu.Root>
            </Show>
            <Show when={isRunning()}>
              <Button
                variant="destructive"
                size="icon-sm"
                class="rounded-full"
                disabled={controlBusy()}
                title={m.chat_stop_title()}
                aria-label={controlBusy() ? m.chat_stopping() : m.chat_stop()}
                onClick={() => void runControl({ action: "cancel" })}
              >
                <Show when={controlBusy()} fallback={<IconBan class="size-4" />}>
                  <IconLoaderCircle class="size-4 animate-spin" />
                </Show>
              </Button>
            </Show>
            <ComposerSendButton
              busy={sending()}
              disabled={sending() || (text().trim() === "" && attachments().length === 0)}
              title={sending() ? m.chat_sending() : isRunning() ? m.chat_queue() : m.chat_send()}
              onClick={() => void send()}
            />
          </>
        }
      >
        <ComposerTextarea
          value={text()}
          onValueChange={updateText}
          onKeyDown={onKeydown}
          onPaste={composer.onPaste}
          onDrop={composer.onDrop}
          rows={2}
          minHeight={52}
          maxHeight={160}
          placeholder={placeholderText()}
          ariaLabel={m.chat_input_label()}
          disabled={sending()}
        />
      </ComposerCard>

      <FolderBrowserDialog
        open={fileBrowserOpen()}
        onOpenChange={setFileBrowserOpen}
        nodeId={props.node?.node_id ?? ""}
        files={true}
        title={m.chat_attach_file()}
        onSelect={composer.addLocalFile}
      />
    </>
  );
}
