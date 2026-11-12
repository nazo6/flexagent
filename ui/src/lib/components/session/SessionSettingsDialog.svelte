<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { SessionModePreference } from "#lib/session-prefs.ts";
  import type { ModeInfo } from "#lib/generated/bindings.ts";
  import { modeChoices } from "#lib/new-session.ts";
  import { m } from "#lib/paraglide/messages.js";
  import BotIcon from "@lucide/svelte/icons/bot";
  import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
  import ServerIcon from "@lucide/svelte/icons/server";
  import Settings2Icon from "@lucide/svelte/icons/settings-2";

  type PathMode = "existing" | "new_worktree";

  let {
    open = $bindable(false),
    pathMode = $bindable<PathMode>("existing"),
    branch = $bindable(""),
    baseBranch = $bindable(""),
    sessionMode = $bindable<SessionModePreference>("default"),
    extraArgsText = $bindable(""),
    manualAgent = $bindable(false),
    agentId = $bindable(""),
    modes = [],
    isProvisionerMode,
    forkActive = false,
    provisionerName,
    testBusy,
    onTest,
    onModeChange,
    onAgentIdInput,
  }: {
    open?: boolean;
    pathMode?: PathMode;
    branch?: string;
    baseBranch?: string;
    sessionMode?: SessionModePreference;
    extraArgsText?: string;
    manualAgent?: boolean;
    agentId?: string;
    /** Modes reported by the probed agent (empty until it answers). */
    modes?: ModeInfo[];
    isProvisionerMode: boolean;
    /** A fork locks the agent to the source session's agent. */
    forkActive?: boolean;
    provisionerName: string;
    testBusy: boolean;
    onTest: () => void;
    onModeChange?: () => void;
    onAgentIdInput?: () => void;
  } = $props();

  /**
   * Initial-mode choices.
   *
   * Only the probed agent's modes are offered: mode IDs are agent-specific, so
   * guessing well-known IDs before the probe answers can send one the agent
   * does not have.
   */
  const modeOptions = $derived(modeChoices(modes));

  /** Value shown by the picker (a saved mode appears once the agent offers it). */
  const modeSelectValue = $derived(
    sessionMode === "default" || modeOptions.some((option) => option.value === sessionMode)
      ? sessionMode
      : "default",
  );
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-xl">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <Settings2Icon class="size-4" />
        {m.new_session_settings_title()}
      </Dialog.Title>
      <Dialog.Description>{m.new_session_settings_description()}</Dialog.Description>
    </Dialog.Header>

    <div class="flex max-h-[60vh] flex-col gap-5 overflow-y-auto pr-1 text-[13px]">
      <section class="flex flex-col gap-2">
        <h3 class="flex items-center gap-1.5 font-medium">
          {#if isProvisionerMode}
            <ServerIcon class="size-3.5" />
            {m.new_session_environment_section()}
          {:else}
            <FolderGit2Icon class="size-3.5" />
            {m.new_session_workspace_section()}
          {/if}
        </h3>

        {#if isProvisionerMode}
          <div class="flex flex-col gap-1">
            <Label>{m.new_session_clone_branch()}</Label>
            <Input
              bind:value={branch}
              placeholder={m.new_session_clone_branch_placeholder()}
              class="h-8 text-[13px]"
            />
          </div>
          <div class="flex items-center justify-between gap-2 rounded-md border p-2">
            <span class="text-muted-foreground">
              {provisionerName || m.new_session_provisioner_undefined()}
            </span>
            <Button
              variant="outline"
              size="sm"
              class="h-7 px-2 text-[13px]"
              disabled={!provisionerName || testBusy}
              onclick={onTest}
            >
              {m.new_session_test()}
            </Button>
          </div>
        {:else}
          <div class="flex items-center gap-1">
            <button
              type="button"
              class="rounded-full border px-2.5 py-0.5 transition-colors {pathMode === 'existing'
                ? 'bg-primary text-primary-foreground border-primary'
                : 'text-muted-foreground hover:bg-muted'}"
              onclick={() => (pathMode = "existing")}
            >
              {m.new_session_existing()}
            </button>
            <button
              type="button"
              class="rounded-full border px-2.5 py-0.5 transition-colors {pathMode ===
              'new_worktree'
                ? 'bg-primary text-primary-foreground border-primary'
                : 'text-muted-foreground hover:bg-muted'}"
              onclick={() => (pathMode = "new_worktree")}
            >
              {m.new_session_new_worktree()}
            </button>
          </div>

          {#if pathMode === "new_worktree"}
            <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
              <div class="flex flex-col gap-1">
                <Label>{m.new_session_new_worktree()}</Label>
                <Input bind:value={branch} placeholder="feat/new-task" class="h-8 text-[13px]" />
              </div>
              <div class="flex flex-col gap-1">
                <Label>{m.new_session_base_branch()}</Label>
                <Input bind:value={baseBranch} placeholder="main" class="h-8 text-[13px]" />
              </div>
            </div>
            <p class="text-muted-foreground text-[11px]">
              {m.new_session_worktree_description()}
            </p>
          {/if}
        {/if}
      </section>

      <section class="flex flex-col gap-2 border-t pt-3">
        <h3 class="flex items-center gap-1.5 font-medium">
          <BotIcon class="size-3.5" />
          {m.new_session_agent_section()}
        </h3>

        <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <div class="flex flex-col gap-1">
            <Label>{m.new_session_initial_mode()}</Label>
            <select
              value={modeSelectValue}
              onchange={(event) => {
                sessionMode = event.currentTarget.value;
                onModeChange?.();
              }}
              class="border-input bg-surface-raised focus-visible:ring-ring/40 h-8 rounded-md border px-2 text-[13px] outline-none focus-visible:ring-1"
            >
              <option value="default">{m.new_session_mode_default()}</option>
              {#each modeOptions as option (option.value)}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </div>
          <div class="flex flex-col gap-1">
            <Label>{m.new_session_extra_args()}</Label>
            <Input
              bind:value={extraArgsText}
              placeholder={m.new_session_extra_args_placeholder()}
              class="h-8 font-mono text-[13px]"
            />
          </div>
        </div>

        <div class="flex flex-col gap-1">
          <div class="flex items-center justify-between gap-2">
            <Label>{m.new_session_agent()}</Label>
            <button
              type="button"
              class="text-muted-foreground hover:text-foreground text-[11px] underline disabled:pointer-events-none disabled:opacity-50"
              disabled={forkActive}
              onclick={() => (manualAgent = !manualAgent)}
            >
              {manualAgent ? m.new_session_choose_from_list() : m.new_session_enter_id()}
            </button>
          </div>
          {#if manualAgent}
            <Input
              bind:value={agentId}
              oninput={onAgentIdInput}
              placeholder={m.new_session_agent_id_placeholder()}
              class="h-8 font-mono text-[13px]"
              disabled={forkActive}
            />
          {:else}
            <p class="text-muted-foreground text-[11px]">{m.new_session_agent_choose_hint()}</p>
          {/if}
        </div>

        <a
          href="/settings?tab=agents"
          class="text-muted-foreground hover:text-foreground text-[11px] underline"
        >
          {m.new_session_manage_agents()}
        </a>
      </section>
    </div>

    <Dialog.Footer>
      <Button variant="outline" size="sm" onclick={() => (open = false)}>
        {m.common_close()}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
