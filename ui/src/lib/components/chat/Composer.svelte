<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import ComposerCard from "#lib/components/composer/ComposerCard.svelte";
  import ComposerSendButton from "#lib/components/composer/ComposerSendButton.svelte";
  import ComposerTextarea from "#lib/components/composer/ComposerTextarea.svelte";
  import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.svelte";
  import PickerItem from "#lib/components/composer/PickerItem.svelte";
  import PickerPill from "#lib/components/composer/PickerPill.svelte";
  import FolderBrowserDialog from "#lib/components/fs/FolderBrowserDialog.svelte";
  import NodeStatusBadge from "#lib/components/NodeStatusBadge.svelte";
  import { encodeBytesToBase64 } from "#lib/base64.ts";
  import { orderedConfigOptions } from "#lib/config-options.ts";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import { saveSessionConfigOption, shouldSubmitOnEnter } from "#lib/session-prefs.ts";
  import { prefs } from "#lib/stores/prefs.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type {
    AttachmentMeta,
    NodeSummary,
    SessionControlAction,
    SessionStatus,
  } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import type { SessionCapabilities } from "#lib/sync/reducer.ts";
  import { toast } from "svelte-sonner";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import BanIcon from "@lucide/svelte/icons/ban";
  import FileIcon from "@lucide/svelte/icons/file";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import Minimize2Icon from "@lucide/svelte/icons/minimize-2";
  import PaperclipIcon from "@lucide/svelte/icons/paperclip";
  import ShieldCheckIcon from "@lucide/svelte/icons/shield-check";
  import SlashIcon from "@lucide/svelte/icons/slash";
  import SlidersHorizontalIcon from "@lucide/svelte/icons/sliders-horizontal";
  import XIcon from "@lucide/svelte/icons/x";

  let {
    sessionId,
    capabilities = null,
    node = null,
    status = null,
    permissionPolicy = null,
    projectId = "",
    /**
     * The last send failed because native restore is unsupported
     * (`RESUME_REQUIRED`). Normally false (sending resumes a stopped session
     * automatically); only when true do we suggest resuming via history replay.
     */
    resumeRequired = $bindable(false),
  }: {
    sessionId: string;
    capabilities?: SessionCapabilities | null;
    node?: NodeSummary | null;
    status?: SessionStatus | null;
    permissionPolicy?: string | null;
    projectId?: string;
    resumeRequired?: boolean;
  } = $props();

  let text = $state("");
  let sending = $state(false);
  let controlBusy = $state(false);
  let resuming = $state(false);
  let attachments = $state<AttachmentMeta[]>([]);
  let fileBrowserOpen = $state(false);
  let fileInput = $state<HTMLInputElement | null>(null);
  let autoApprove = $state(false);
  const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

  $effect(() => {
    autoApprove = permissionPolicy === "auto_allow_all";
  });

  async function toggleAutoApprove() {
    const policy = autoApprove ? "ask" : "auto_allow_all";
    const result = await sync.controlSession(sessionId, {
      action: "set_permission_policy",
      policy,
    });
    if (!result.success) {
      if (result.error) problems.reportInfo("permission policy", result.error);
      return;
    }
    autoApprove = policy === "auto_allow_all";
    toast.info(autoApprove ? m.chat_auto_approve_enabled() : m.chat_auto_approve_disabled());
  }

  const isRunning = $derived(
    status === "running" || status === "waiting_permission" || status === "waiting_input",
  );
  const isStopped = $derived(status === "stopped" || status === "error");

  // Close the resume suggestion once the session becomes active
  $effect(() => {
    if (
      status === "idle" ||
      status === "running" ||
      status === "waiting_permission" ||
      status === "waiting_input"
    ) {
      resumeRequired = false;
    }
  });

  const nodeAvail = $derived(getNodeAvailability(node));

  /** Mode selector choices (value = mode_id, label = the name reported by the agent). */
  const modeItems = $derived(
    (capabilities?.availableModes ?? []).map((mode) => ({
      value: mode.mode_id,
      label: mode.name.trim() === "" ? mode.mode_id : mode.name,
      description: mode.description,
    })),
  );

  /** Current mode (an ID that is not among the choices is treated as unselected). */
  const currentMode = $derived(capabilities?.currentMode ?? null);
  const selectedMode = $derived(
    currentMode !== null && modeItems.some((item) => item.value === currentMode)
      ? currentMode
      : undefined,
  );
  const selectedModeLabel = $derived(
    modeItems.find((item) => item.value === selectedMode)?.label ?? "",
  );

  /** Agent config options (model / thought level / agent-specific), newest first. */
  const configOptions = $derived(orderedConfigOptions(capabilities?.configOptions ?? []));

  const slashQuery = $derived(
    text.startsWith("/") && !text.includes(" ") ? text.slice(1).toLowerCase() : null,
  );

  const commandMatches = $derived(
    capabilities && slashQuery !== null
      ? capabilities.availableCommands
          .filter((command) => command.name.toLowerCase().startsWith(slashQuery))
          .slice(0, 8)
      : [],
  );

  /** Whether context compaction (`/compact`) is supported (detected via the synthetic entry). */
  const canCompact = $derived(
    capabilities?.availableCommands.some((command) => command.name === "compact") ?? false,
  );

  function applyCommand(name: string) {
    text = `/${name} `;
  }

  /** Best-effort MIME type from a file name (mirrors the CLI helper). */
  function guessMime(fileName: string): string {
    const extension = fileName.split(".").pop()?.toLowerCase() ?? "";
    switch (extension) {
      case "png":
        return "image/png";
      case "jpg":
      case "jpeg":
        return "image/jpeg";
      case "gif":
        return "image/gif";
      case "webp":
        return "image/webp";
      case "svg":
        return "image/svg+xml";
      case "pdf":
        return "application/pdf";
      case "json":
        return "application/json";
      case "md":
        return "text/markdown";
      case "txt":
      case "log":
        return "text/plain";
      default:
        return "application/octet-stream";
    }
  }

  async function addImageFiles(files: FileList | File[]) {
    for (const file of Array.from(files)) {
      if (!file.type.startsWith("image/")) {
        toast.error(m.chat_attachment_image_only());
        continue;
      }
      if (file.size > MAX_ATTACHMENT_BYTES) {
        toast.error(m.chat_attachment_too_large({ limit: MAX_ATTACHMENT_BYTES / (1024 * 1024) }));
        continue;
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      attachments.push({
        file_name: file.name || "image",
        mime_type: file.type || guessMime(file.name),
        size_bytes: file.size,
        local_path: null,
        data_b64: encodeBytesToBase64(bytes),
      });
    }
  }

  function addLocalFile(path: string) {
    const fileName = path.split(/[\\/]/).pop() ?? "attachment";
    attachments.push({
      file_name: fileName,
      mime_type: guessMime(fileName),
      size_bytes: 0,
      local_path: path,
      data_b64: null,
    });
  }

  function removeAttachment(index: number) {
    attachments = attachments.filter((_, current) => current !== index);
  }

  function onPaste(event: ClipboardEvent) {
    const files = event.clipboardData?.files;
    if (files && files.length > 0) {
      event.preventDefault();
      void addImageFiles(files);
    }
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    const files = event.dataTransfer?.files;
    if (files && files.length > 0) {
      void addImageFiles(files);
    }
  }

  function onFileInputChange(event: Event) {
    const files = (event.currentTarget as HTMLInputElement).files;
    if (files) {
      void addImageFiles(files);
      (event.currentTarget as HTMLInputElement).value = "";
    }
  }

  async function send() {
    const value = text.trim();
    if ((value === "" && attachments.length === 0) || sending) return;
    sending = true;
    try {
      const result = await sync.sendPrompt(sessionId, {
        text: value,
        attachments: [...attachments],
      });
      if (!result.success) {
        if (result.error?.code === "RESUME_REQUIRED") {
          // Automatic resume (native restore) is unsupported for stopped
          // sessions. Keep the input and suggest resuming via history replay.
          resumeRequired = true;
        } else if (result.error) {
          problems.reportInfo("send prompt", result.error);
        } else {
          problems.warn("send prompt", m.chat_send_failed());
        }
        return;
      }
      text = "";
      attachments = [];
      resumeRequired = false;
    } catch (error) {
      // Keep the composer text so the user can retry without retyping.
      problems.report("send prompt", error);
    } finally {
      sending = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (shouldSubmitOnEnter(event, prefs.sendShortcut)) {
      event.preventDefault();
      void send();
    }
  }

  async function runControl(action: SessionControlAction) {
    if (controlBusy) return;
    controlBusy = true;
    try {
      const result = await sync.controlSession(sessionId, action);
      if (!result.success) {
        if (result.error) {
          problems.reportInfo("session control", result.error);
        } else {
          problems.warn("session control", m.chat_control_failed());
        }
      } else if (action.action === "cancel") {
        toast.info(m.chat_cancel_requested());
      } else if (action.action === "set_config" && projectId !== "") {
        saveSessionConfigOption(projectId, action.key, action.value);
      }
    } catch (error) {
      problems.report("session control", error);
    } finally {
      controlBusy = false;
    }
  }

  async function requestCompact() {
    if (sending || controlBusy) return;
    try {
      const result = await sync.sendPrompt(sessionId, {
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
    if (sending || resuming) return false;
    resuming = true;
    try {
      const result = await sync.resumeSession(sessionId);
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
      resuming = false;
    }
  }

  /** Resume a stopped session without native restore via history replay, then send the input. */
  async function resumeAndSend() {
    if (sending || (text.trim() === "" && attachments.length === 0)) return;
    if (!(await resumeSession())) return;
    resumeRequired = false;
    await send();
  }
</script>

{#snippet modeIcon()}
  <SlidersHorizontalIcon class="text-muted-foreground size-3.5" />
{/snippet}

<ComposerCard>
  {#snippet above()}
    <div class="flex flex-col gap-2 px-1 pt-1">
      {#if isStopped}
        <div
          class="border-warn/25 bg-warn-soft/40 text-warn flex flex-wrap items-center gap-2 rounded-md border px-3 py-1.5 text-[13px]"
        >
          <AlertTriangleIcon class="size-3.5 shrink-0" />
          <span>
            {m.chat_stopped_notice()}
          </span>
          <Button
            variant="outline"
            size="sm"
            class="ml-auto h-6 text-[11px]"
            disabled={sending || resuming}
            title={m.chat_resume_title()}
            onclick={() => void resumeSession()}
          >
            {#if resuming}
              <LoaderCircleIcon class="size-3.5 animate-spin" />
            {/if}
            {resuming ? m.chat_resuming() : m.chat_resume()}
          </Button>
        </div>
      {:else if node && !nodeAvail.isAvailable}
        <div
          class="border-danger/25 bg-danger-soft/50 text-danger flex items-center gap-2 rounded-md border px-3 py-1.5 text-[13px]"
        >
          <AlertTriangleIcon class="size-3.5 shrink-0" />
          <span>
            {m.chat_node_unavailable({ name: node.name, status: nodeAvail.statusText })}
          </span>
        </div>
      {/if}

      {#if resumeRequired}
        <div
          class="border-warn/25 bg-warn-soft/40 text-warn flex flex-wrap items-center gap-2 rounded-md border px-3 py-1.5 text-[13px]"
        >
          <AlertTriangleIcon class="size-3.5 shrink-0" />
          <span>
            {m.chat_resume_required_notice()}
          </span>
          <Button
            variant="outline"
            size="sm"
            class="ml-auto h-6 text-[11px]"
            disabled={sending || resuming}
            onclick={resumeAndSend}
          >
            {#if resuming}
              <LoaderCircleIcon class="size-3.5 animate-spin" />
            {/if}
            {resuming ? m.chat_resuming() : m.chat_resume_and_send()}
          </Button>
        </div>
      {/if}

      {#if commandMatches.length > 0}
        <div class="flex flex-wrap gap-1">
          {#each commandMatches as command (command.name)}
            <button
              type="button"
              class="bg-chip hover:bg-chip-hover border-border flex items-center gap-1 rounded-md border px-2 py-1 text-[13px]"
              onclick={() => applyCommand(command.name)}
            >
              <SlashIcon class="size-3" />
              <span class="font-mono">{command.name}</span>
              <span class="text-muted-foreground">{command.description}</span>
            </button>
          {/each}
        </div>
      {/if}

      {#if attachments.length > 0}
        <div class="flex flex-wrap gap-2">
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
                onclick={() => removeAttachment(index)}
              >
                <XIcon class="size-3.5" />
              </button>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/snippet}

  {#snippet toolbar()}
    <input
      bind:this={fileInput}
      type="file"
      accept="image/*"
      multiple
      class="hidden"
      onchange={onFileInputChange}
    />
    <div class="flex min-w-0 flex-1 items-center gap-1">
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
      {#if node}
        <Button
          variant="ghost"
          size="icon-sm"
          class="rounded-full"
          title={m.chat_attach_file()}
          aria-label={m.chat_attach_file()}
          onclick={() => (fileBrowserOpen = true)}
        >
          <FileIcon class="size-4" />
        </Button>
      {/if}
      {#if node}
        <NodeStatusBadge
          {node}
          showNodeName={true}
          showLabel={false}
          class="hidden shrink-0 sm:inline-flex"
        />
      {/if}
      {#if capabilities && capabilities.availableModes.length > 0}
        <PickerPill
          label={m.chat_mode_label()}
          value={selectedModeLabel}
          icon={modeIcon}
          contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
        >
          <div class="max-h-72 overflow-y-auto p-1">
            {#each modeItems as item (item.value)}
              <PickerItem
                selected={item.value === selectedMode}
                onselect={() => void runControl({ action: "set_mode", mode_id: item.value })}
              >
                <span class="block truncate">{item.label}</span>
                {#if item.description}
                  <span class="text-muted-foreground block truncate text-[11px]">
                    {item.description}
                  </span>
                {/if}
              </PickerItem>
            {/each}
          </div>
        </PickerPill>
      {/if}

      {#each configOptions as option (option.key)}
        <ConfigOptionPicker
          {option}
          onchange={(value) =>
            void runControl({
              action: "set_config",
              key: option.key,
              value,
            })}
        />
      {/each}
    </div>

    <div class="ml-1 flex shrink-0 items-center gap-1">
      {#if !sync.wsConnected}
        <span class="text-danger text-[11px]">{m.chat_connection_lost()}</span>
      {/if}
      <Button
        variant="outline"
        size="sm"
        class={`h-7 rounded-full px-2.5 text-xs ${
          autoApprove ? "border-signal/40 bg-signal-soft text-signal hover:bg-signal-soft" : ""
        }`}
        disabled={controlBusy || sending}
        title={m.chat_auto_approve_title()}
        onclick={() => void toggleAutoApprove()}
      >
        <ShieldCheckIcon class="size-3.5" />
        {m.chat_auto_approve()}
      </Button>
      {#if canCompact && !isStopped}
        <Button
          variant="ghost"
          size="sm"
          class="h-7 rounded-full px-2 text-xs"
          disabled={controlBusy || sending}
          title={m.chat_compact_title()}
          onclick={() => void requestCompact()}
        >
          <Minimize2Icon class="size-3.5" />
          <span class="hidden sm:inline">{m.chat_compact()}</span>
        </Button>
      {/if}
      {#if isRunning}
        <Button
          variant="destructive"
          size="sm"
          class="h-7 rounded-full px-2.5 text-xs"
          disabled={controlBusy}
          title={m.chat_stop_title()}
          onclick={() => void runControl({ action: "cancel" })}
        >
          <BanIcon class="size-3.5" />
          {controlBusy ? m.chat_stopping() : m.chat_stop()}
        </Button>
      {/if}
      <ComposerSendButton
        busy={sending}
        disabled={sending || (text.trim() === "" && attachments.length === 0)}
        title={sending ? m.chat_sending() : isRunning ? m.chat_queue() : m.chat_send()}
        onclick={() => void send()}
      />
    </div>
  {/snippet}

  <ComposerTextarea
    bind:value={text}
    onkeydown={onKeydown}
    onpaste={onPaste}
    ondrop={onDrop}
    rows={3}
    placeholder={isStopped
      ? m.chat_placeholder_stopped()
      : isRunning
        ? m.chat_placeholder_running()
        : m.chat_placeholder_idle()}
    disabled={sending}
  />
</ComposerCard>

<FolderBrowserDialog
  bind:open={fileBrowserOpen}
  nodeId={node?.node_id ?? ""}
  files={true}
  title={m.chat_attach_file()}
  onSelect={addLocalFile}
/>
