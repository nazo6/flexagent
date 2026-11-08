<script lang="ts">
  import { Button } from "#lib/components/ui/button/index.ts";
  import ComposerSelect from "#lib/components/chat/ComposerSelect.svelte";
  import NodeStatusBadge from "#lib/components/NodeStatusBadge.svelte";
  import { configChoices, resolveConfigChoice, selectedConfigValue } from "#lib/config-options.ts";
  import { getNodeAvailability } from "#lib/node-status.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { commandErrorMessage } from "#lib/api/client.ts";
  import type {
    NodeSummary,
    SessionControlAction,
    SessionStatus,
  } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import type { SessionCapabilities } from "#lib/sync/reducer.ts";
  import { toast } from "svelte-sonner";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import BanIcon from "@lucide/svelte/icons/ban";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import Minimize2Icon from "@lucide/svelte/icons/minimize-2";
  import SendIcon from "@lucide/svelte/icons/send";
  import SlashIcon from "@lucide/svelte/icons/slash";

  let {
    sessionId,
    capabilities = null,
    node = null,
    status = null,
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
    resumeRequired?: boolean;
  } = $props();

  let text = $state("");
  let sending = $state(false);
  let controlBusy = $state(false);
  let resuming = $state(false);

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

  async function send() {
    const value = text.trim();
    if (!value || sending) return;
    sending = true;
    try {
      const result = await sync.sendPrompt(sessionId, value);
      if (!result.success) {
        if (result.error?.code === "RESUME_REQUIRED") {
          // Automatic resume (native restore) is unsupported for stopped
          // sessions. Keep the input and suggest resuming via history replay.
          resumeRequired = true;
        } else {
          toast.error(commandErrorMessage(result.error) ?? m.chat_send_failed());
        }
        return;
      }
      text = "";
      resumeRequired = false;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      sending = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
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
        toast.error(commandErrorMessage(result.error) ?? m.chat_control_failed());
      } else if (action.action === "cancel") {
        toast.info(m.chat_cancel_requested());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      controlBusy = false;
    }
  }

  async function requestCompact() {
    if (sending || controlBusy) return;
    try {
      const result = await sync.sendPrompt(sessionId, "/compact");
      if (!result.success) {
        toast.error(commandErrorMessage(result.error) ?? m.chat_compact_failed());
      } else {
        toast.info(m.chat_compact_sent());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
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
      toast.error(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      resuming = false;
    }
  }

  /** Resume a stopped session without native restore via history replay, then send the input. */
  async function resumeAndSend() {
    if (sending || text.trim() === "") return;
    if (!(await resumeSession())) return;
    resumeRequired = false;
    await send();
  }
</script>

<div class="flex flex-col gap-2 border-t pt-2">
  {#if isStopped}
    <div
      class="bg-muted text-muted-foreground flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1 text-xs"
    >
      <AlertTriangleIcon class="size-3.5 shrink-0" />
      <span>
        {m.chat_stopped_notice()}
      </span>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-6 text-xs"
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
      class="bg-destructive/10 text-destructive flex items-center gap-2 rounded-md px-2.5 py-1 text-xs"
    >
      <AlertTriangleIcon class="size-3.5 shrink-0" />
      <span>
        {m.chat_node_unavailable({ name: node.name, status: nodeAvail.statusText })}
      </span>
    </div>
  {/if}

  {#if resumeRequired}
    <div
      class="bg-muted text-muted-foreground flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1 text-xs"
    >
      <AlertTriangleIcon class="size-3.5 shrink-0 text-amber-500/80" />
      <span>
        {m.chat_resume_required_notice()}
      </span>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-6 text-xs"
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
          class="bg-muted hover:bg-accent flex items-center gap-1 rounded-md px-2 py-1 text-xs"
          onclick={() => applyCommand(command.name)}
        >
          <SlashIcon class="size-3" />
          <span class="font-mono">{command.name}</span>
          <span class="text-muted-foreground">{command.description}</span>
        </button>
      {/each}
    </div>
  {/if}

  <textarea
    bind:value={text}
    onkeydown={onKeydown}
    rows="3"
    placeholder={isStopped
      ? m.chat_placeholder_stopped()
      : isRunning
        ? m.chat_placeholder_running()
        : m.chat_placeholder_idle()}
    class="border-input bg-background focus-visible:ring-ring/50 w-full resize-y rounded-md border px-3 py-2 text-sm outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
    disabled={sending}></textarea>

  <div class="flex flex-wrap items-center gap-2">
    {#if node}
      <NodeStatusBadge {node} showNodeName={true} class="mr-1 hidden sm:inline-flex" />
    {/if}
    {#if capabilities && capabilities.availableModes.length > 0}
      <ComposerSelect
        label={m.chat_mode_label()}
        placeholder={m.chat_default_placeholder()}
        value={selectedMode}
        items={modeItems}
        onchange={(value) => void runControl({ action: "set_mode", mode_id: value })}
      />
    {/if}

    {#each capabilities?.configOptions ?? [] as option (option.key)}
      {@const choices = configChoices(option.options)}
      {#if choices.length > 0}
        <ComposerSelect
          label={option.name}
          placeholder={m.chat_default_placeholder()}
          value={selectedConfigValue(choices, option.current_value)}
          items={choices.map((choice) => ({
            value: choice.value,
            label: choice.label,
          }))}
          onchange={(value) =>
            void runControl({
              action: "set_config",
              key: option.key,
              value: resolveConfigChoice(choices, value),
            })}
        />
      {/if}
    {/each}

    <div class="ml-auto flex items-center gap-2">
      {#if !sync.wsConnected}
        <span class="text-destructive text-xs">{m.chat_connection_lost()}</span>
      {/if}
      {#if canCompact && !isStopped}
        <Button
          variant="outline"
          size="sm"
          disabled={controlBusy || sending}
          title={m.chat_compact_title()}
          onclick={() => void requestCompact()}
        >
          <Minimize2Icon class="size-3.5" />
          {m.chat_compact()}
        </Button>
      {/if}
      {#if isRunning}
        <Button
          variant="destructive"
          size="sm"
          disabled={controlBusy}
          title={m.chat_stop_title()}
          onclick={() => void runControl({ action: "cancel" })}
        >
          <BanIcon class="size-3.5" />
          {controlBusy ? m.chat_stopping() : m.chat_stop()}
        </Button>
      {/if}
      <Button size="sm" disabled={sending || text.trim() === ""} onclick={() => void send()}>
        {#if sending}
          <LoaderCircleIcon class="size-3.5 animate-spin" />
        {:else}
          <SendIcon class="size-3.5" />
        {/if}
        {sending ? m.chat_sending() : isRunning ? m.chat_queue() : m.chat_send()}
      </Button>
    </div>
  </div>
</div>
