<script lang="ts">
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import BootstrapLogCard from "#lib/components/BootstrapLogCard.svelte";
  import ChatTimeline from "#lib/components/chat/ChatTimeline.svelte";
  import Composer from "#lib/components/chat/Composer.svelte";
  import DiffPane from "#lib/components/diff/DiffPane.svelte";
  import NodeStatusBadge from "#lib/components/NodeStatusBadge.svelte";
  import SessionActionsMenu from "#lib/components/session/SessionActionsMenu.svelte";
  import ContextMeter from "#lib/components/session/ContextMeter.svelte";
  import SessionStatusBadge from "#lib/components/SessionStatusBadge.svelte";
  import TerminalView from "#lib/components/terminal/TerminalView.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { commandErrorMessage } from "#lib/api/client.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { formatRelativeTime } from "#lib/format.ts";
  import { shortId } from "#lib/session-status.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import {
    bootstrapLogLines,
    buildTimelineItems,
    capabilitiesFromEvents,
    latestStatusFromEvents,
    latestUsageFromEvents,
  } from "#lib/sync/reducer.ts";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
  import ChevronLeftIcon from "@lucide/svelte/icons/chevron-left";
  import FileCodeIcon from "@lucide/svelte/icons/file-code";
  import FolderIcon from "@lucide/svelte/icons/folder";
  import GitForkIcon from "@lucide/svelte/icons/git-fork";
  import MenuIcon from "@lucide/svelte/icons/menu";
  import MessageSquareIcon from "@lucide/svelte/icons/message-square";
  import PanelLeftOpenIcon from "@lucide/svelte/icons/panel-left-open";
  import PanelRightCloseIcon from "@lucide/svelte/icons/panel-right-close";
  import PlayIcon from "@lucide/svelte/icons/play";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import SkullIcon from "@lucide/svelte/icons/skull";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

  type ViewMode = "chat" | "diff" | "terminal";

  const sessionId = $derived(page.params.id ?? "");
  const session = $derived(sync.sessions.find((entry) => entry.session_id === sessionId) ?? null);
  const sessionNode = $derived(
    session ? (sync.nodes.find((n) => n.node_id === session.node_id) ?? null) : null,
  );

  const timeline = $derived(sync.timelineFor(sessionId));
  const currentStatus = $derived(
    latestStatusFromEvents(timeline.events)?.status ?? session?.status ?? null,
  );
  const items = $derived(
    buildTimelineItems({
      sessionId,
      events: timeline.events,
      messageDeltas: timeline.messageDeltas,
      thoughtDeltas: timeline.thoughtDeltas,
      toolProgress: timeline.toolProgress,
      terminalDeltas: timeline.terminalDeltas,
      streamKeys: timeline.streamKeys,
      pendingPrompts: sync.pendingPrompts,
    }),
  );
  const capabilities = $derived(capabilitiesFromEvents(timeline.events));
  // Prefer the latest usage derived from events, and fall back to the session list (projection) value.
  const usage = $derived(latestUsageFromEvents(timeline.events) ?? session?.usage ?? null);
  // Merge persistent events (BootstrapLog synced after a turn completes) with the
  // ephemeral log the server streams from an ephemeral VM.
  const bootstrapLines = $derived.by(() => {
    const persisted = bootstrapLogLines(timeline.events);
    const live = sync.bootstrapLinesFor(sessionId);
    return live.length === 0 ? persisted : [...new Set([...persisted, ...live])];
  });

  let chatContainer = $state<HTMLDivElement | null>(null);
  let loadedFor = $state<string | null>(null);
  let showScrollBottom = $state(false);

  // View mode (mobile tab switch and desktop right-pane selection)
  let activeTab = $state<ViewMode>("chat");
  let sidePanelMode = $state<"diff" | "terminal" | null>(null);

  // Desktop resizable right pane
  let mainContainer = $state<HTMLDivElement | null>(null);
  let sidePanelWidth = $state(42); // default percentage
  let isResizing = $state(false);

  onMount(() => {
    if (typeof localStorage !== "undefined") {
      const saved = localStorage.getItem("fxg:session_side_panel_width");
      if (saved) {
        const num = parseFloat(saved);
        if (!isNaN(num) && num >= 20 && num <= 80) {
          sidePanelWidth = num;
        }
      }
    }
  });

  function handlePointerMove(e: PointerEvent) {
    if (!isResizing || !mainContainer) return;
    const rect = mainContainer.getBoundingClientRect();
    const pointerX = e.clientX;
    const rightPixels = rect.right - pointerX;
    const percentage = (rightPixels / rect.width) * 100;
    // Clamp between 20% and 75%
    sidePanelWidth = Math.max(20, Math.min(75, Math.round(percentage)));
  }

  function handlePointerUp() {
    if (!isResizing) return;
    isResizing = false;
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", handlePointerUp);
    if (typeof localStorage !== "undefined") {
      try {
        localStorage.setItem("fxg:session_side_panel_width", String(sidePanelWidth));
      } catch {
        // Ignore quota errors
      }
    }
  }

  function startResize(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    isResizing = true;
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
  }

  // Make the session a priority target for WS delivery, and fall back to REST
  // loading when the history has not been fetched yet.
  $effect(() => {
    const id = sessionId;
    if (!id) return;
    sync.setActiveSession(id);
    if (loadedFor !== id) {
      loadedFor = id;
      const target = sync.timelineFor(id);
      if (target.events.length === 0) {
        sync.loadSessionEvents(id).catch(() => {
          /* Ignore: the WS replay will make it visible once it arrives. */
        });
      }
    }
    return () => sync.setActiveSession(null);
  });

  // Scroll to the end when a new item is added (unless the user is browsing the past)
  $effect(() => {
    const currentItems = items;
    if (chatContainer !== null && currentItems.length > 0 && !showScrollBottom) {
      chatContainer.scrollTop = chatContainer.scrollHeight;
    }
  });

  function handleScroll() {
    if (!chatContainer) return;
    const distanceToBottom =
      chatContainer.scrollHeight - chatContainer.scrollTop - chatContainer.clientHeight;
    showScrollBottom = distanceToBottom > 150;
  }

  let killDialogOpen = $state(false);
  let killing = $state(false);

  // Execution state of Resume (restarting a stopped session)
  let resuming = $state(false);

  /**
   * Whether native restore is unsupported (`RESUME_REQUIRED`) was detected.
   *
   * Normally a stopped session is resumed automatically on send, so the
   * Resume button is not shown. It is only shown when sending failed with
   * `RESUME_REQUIRED` (i.e. resuming via history replay is required). Shared with Composer.
   * Even when `status` diverges from reality (for example after a forced kill),
   * the automatic resume on send restarts based on the actual state (or shows this
   * button via `RESUME_REQUIRED`).
   */
  let resumeRequired = $state(false);

  /** Show Resume only for sessions where unsupported native restore was detected. */
  const canResume = $derived(resumeRequired && session !== null);

  function scrollToBottom() {
    if (!chatContainer) return;
    chatContainer.scrollTo({
      top: chatContainer.scrollHeight,
      behavior: "smooth",
    });
    showScrollBottom = false;
  }

  function toggleSidePanel(mode: "diff" | "terminal") {
    if (sidePanelMode === mode) {
      sidePanelMode = null;
    } else {
      sidePanelMode = mode;
    }
  }

  function handleForkSession() {
    if (!sessionId) return;
    const params = new URLSearchParams();
    params.set("fork_session", sessionId);
    if (session?.project_id) params.set("project", session.project_id);
    if (session?.agent_id) params.set("agent", session.agent_id);
    // Start the fork on the same node and working directory (overrides default path resolution)
    if (session?.node_id) params.set("node", session.node_id);
    if (session?.local_path) params.set("path", session.local_path);
    void goto(`/?${params.toString()}`);
  }

  /** Open a new session on the same node and working directory as this session. */
  function handleNewSessionHere() {
    if (!session) return;
    const params = new URLSearchParams();
    params.set("project", session.project_id);
    params.set("node", session.node_id);
    params.set("path", session.local_path);
    if (session.agent_id) params.set("agent", session.agent_id);
    void goto(`/?${params.toString()}`);
  }

  /** Resume a stopped session via history replay (`POST /sessions/:id/resume`). */
  async function handleResumeSession() {
    if (!sessionId || resuming) return;
    resuming = true;
    try {
      const result = await sync.resumeSession(sessionId);
      resumeRequired = false;
      if (result.context_restored) {
        toast.success(m.session_resume_success_context());
      } else {
        toast.success(m.session_resume_success_replay());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      resuming = false;
    }
  }

  async function handleKillSession() {
    if (!sessionId || killing) return;
    killing = true;
    try {
      const result = await sync.controlSession(sessionId, { action: "kill" });
      if (result.success) {
        toast.success(m.session_kill_success());
        killDialogOpen = false;
        await sync.refreshSessions();
      } else {
        toast.error(commandErrorMessage(result.error) ?? m.session_kill_failed());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      killing = false;
    }
  }

  /** Unarchive the session (from the archived banner). */
  async function handleUnarchive() {
    if (!sessionId) return;
    try {
      await sync.archiveSession(sessionId, false);
      toast.success(m.session_unarchived());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }
</script>

<svelte:head>
  <title>{session?.title ?? m.session_title_fallback()} - FlexAgent</title>
</svelte:head>

<div class="flex h-full w-full flex-col overflow-hidden bg-background">
  <!-- Session header -->
  <header class="bg-card/85 border-b border-border px-4 py-2.5 shrink-0 backdrop-blur">
    <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
      <!-- Left: back link + title + node status -->
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <!-- Mobile: open sidebar drawer to pick sessions -->
        <button
          type="button"
          class="hover:bg-accent text-muted-foreground hover:text-foreground -ml-1 flex size-7 items-center justify-center rounded-md border md:hidden"
          onclick={() => layoutState.setMobileDrawer(true)}
          title={m.shell_open_menu()}
          aria-label={m.shell_open_menu()}
        >
          <MenuIcon class="size-4" />
        </button>

        <!-- Desktop: expand sidebar when collapsed -->
        {#if layoutState.sidebarCollapsed}
          <button
            type="button"
            class="hover:bg-accent text-muted-foreground hover:text-foreground -ml-1 hidden size-7 items-center justify-center rounded-md border md:flex"
            onclick={() => layoutState.toggleSidebar()}
            title="Open sidebar (Ctrl+B)"
            aria-label="Open sidebar"
          >
            <PanelLeftOpenIcon class="size-4" />
          </button>
        {/if}

        <div class="flex min-w-0 flex-col gap-0.5">
          <div class="flex items-center gap-2">
            <h1 class="truncate text-[15px] font-semibold tracking-tight">
              {session?.title ?? m.session_title_fallback()}
            </h1>
            {#if currentStatus}
              <SessionStatusBadge status={currentStatus} />
            {/if}
          </div>

          <div class="text-muted-foreground flex flex-wrap items-center gap-x-2 text-[11px]">
            <!-- Node availability status -->
            <NodeStatusBadge node={sessionNode} showNodeName={true} />
            <span class="opacity-40">/</span>
            <span class="truncate">{session?.project_id}</span>
            {#if session?.git_branch}
              <span class="opacity-40">/</span>
              <span class="font-mono">{session.git_branch}</span>
            {/if}
            {#if session?.is_worktree}
              <span class="bg-muted rounded px-1 text-[11px]">wt</span>
            {/if}
            {#if session?.local_path}
              <span class="opacity-40">/</span>
              <button
                type="button"
                class="hover:text-foreground inline-flex items-center gap-1 font-mono text-[11px] truncate max-w-44 sm:max-w-xs md:max-w-md transition-colors cursor-pointer"
                title={m.session_working_dir_copy_title({ path: session.local_path })}
                onclick={() => {
                  navigator.clipboard.writeText(session.local_path);
                  toast.success(m.session_working_dir_copied());
                }}
              >
                <FolderIcon class="text-signal size-3.5 shrink-0" />
                <span class="truncate">{session.local_path}</span>
              </button>
            {/if}
            <span class="opacity-40">/</span>
            <span>{session?.agent_id}</span>
            {#if session?.updated_at}
              <span class="opacity-40">/</span>
              <span>{formatRelativeTime(session.updated_at)}</span>
            {/if}
            {#if usage}
              <span class="opacity-40">/</span>
              <ContextMeter {usage} />
            {/if}
          </div>
        </div>
      </div>

      <!-- Right: mode switch / pane toggle buttons -->
      <div class="flex items-center gap-1.5">
        <!-- Mobile: tab switch buttons -->
        <div class="flex rounded-md border bg-muted/30 p-0.5 md:hidden">
          <button
            type="button"
            class={cn(
              "px-2.5 py-1 text-[13px] font-medium rounded",
              activeTab === "chat" ? "bg-background text-foreground" : "text-muted-foreground",
            )}
            onclick={() => (activeTab = "chat")}
          >
            {m.session_tab_chat()}
          </button>
          <button
            type="button"
            class={cn(
              "px-2.5 py-1 text-[13px] font-medium rounded",
              activeTab === "diff" ? "bg-background text-foreground" : "text-muted-foreground",
            )}
            onclick={() => (activeTab = "diff")}
          >
            Diff
          </button>
          <button
            type="button"
            class={cn(
              "px-2.5 py-1 text-[13px] font-medium rounded",
              activeTab === "terminal" ? "bg-background text-foreground" : "text-muted-foreground",
            )}
            onclick={() => (activeTab = "terminal")}
          >
            Terminal
          </button>
        </div>

        <!-- Desktop: side panel toggles -->
        <div class="hidden items-center gap-1 md:flex">
          <Button
            variant={sidePanelMode === "diff" ? "secondary" : "ghost"}
            size="sm"
            class="h-7 text-[13px] gap-1.5"
            onclick={() => toggleSidePanel("diff")}
            title={m.session_diff_panel_toggle_title()}
          >
            <FileCodeIcon class="size-3.5" />
            <span>Diff</span>
          </Button>

          <Button
            variant={sidePanelMode === "terminal" ? "secondary" : "ghost"}
            size="sm"
            class="h-7 text-[13px] gap-1.5"
            onclick={() => toggleSidePanel("terminal")}
            title={m.session_terminal_panel_toggle_title()}
          >
            <SquareTerminalIcon class="size-3.5" />
            <span>Terminal</span>
          </Button>
        </div>

        <div class="h-4 w-px bg-border/60 mx-0.5 hidden sm:block"></div>

        <!-- Resume button (only for stopped sessions without native restore support) -->
        {#if canResume}
          <Button
            variant="ghost"
            size="sm"
            class="h-7 text-[13px] gap-1"
            disabled={resuming}
            onclick={handleResumeSession}
            title={m.session_resume_button_title()}
          >
            <PlayIcon class="size-3" />
            <span class="hidden sm:inline">Resume</span>
          </Button>
        {/if}

        <!-- Fork button (only when the agent supports ACP-native session/fork) -->
        {#if session?.fork_supported}
          <Button
            variant="ghost"
            size="sm"
            class="h-7 text-[13px] gap-1"
            onclick={handleForkSession}
            title={m.session_fork_button_title()}
          >
            <GitForkIcon class="size-3" />
            <span class="hidden sm:inline">Fork</span>
          </Button>
        {/if}

        <!-- New session in the same place (without carrying over history) -->
        {#if session && sessionNode && !sessionNode.is_ephemeral}
          <Button
            variant="ghost"
            size="sm"
            class="h-7 text-[13px] gap-1"
            onclick={handleNewSessionHere}
            title={m.session_new_here_button_title()}
          >
            <PlusIcon class="size-3" />
            <span class="hidden sm:inline">{m.session_new_here()}</span>
          </Button>
        {/if}

        <!-- Kill button (forced termination) -->
        <Button
          variant="ghost"
          size="sm"
          class="h-7 text-[13px] gap-1 text-destructive hover:bg-destructive/10 hover:text-destructive"
          onclick={() => (killDialogOpen = true)}
          title={m.session_kill_button_title()}
        >
          <SkullIcon class="size-3" />
          <span class="hidden sm:inline">Kill</span>
        </Button>

        <!-- Archive / delete menu -->
        {#if session}
          <SessionActionsMenu {session} onDeleted={() => void goto("/")} />
        {/if}
      </div>
    </div>
  </header>

  <!-- Archived banner -->
  {#if session && session.archived_at !== null}
    <div
      class="border-warn/30 bg-warn-soft/50 flex shrink-0 items-center justify-between gap-2 border-b px-4 py-2 text-[13px]"
    >
      <div class="text-warn flex items-center gap-2">
        <ArchiveIcon class="size-4" />
        <span>{m.session_archived_banner()}</span>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="h-7 shrink-0 gap-1 text-[11px]"
        onclick={handleUnarchive}
      >
        {m.session_unarchive()}
      </Button>
    </div>
  {/if}

  <!-- Kill confirmation modal -->
  <AlertDialog.Root bind:open={killDialogOpen}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title class="flex items-center gap-2 text-destructive">
          <SkullIcon class="size-5" />
          {m.session_kill_confirm_title()}
        </AlertDialog.Title>
        <AlertDialog.Description class="text-sm">
          {m.session_kill_confirm_description_pre()}
          <code>{sessionId}</code>{m.session_kill_confirm_description_post()}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel disabled={killing}>{m.common_cancel()}</AlertDialog.Cancel>
        <AlertDialog.Action
          class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
          disabled={killing}
          onclick={handleKillSession}
        >
          {killing ? m.session_killing() : m.session_kill_action()}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>

  <!-- Error display when the session does not exist -->
  {#if session === null}
    <div class="p-4 text-center text-sm text-muted-foreground">
      {m.session_not_found()}
    </div>
  {/if}

  <!-- Main content area -->
  <div
    bind:this={mainContainer}
    class="flex flex-1 overflow-hidden relative"
    class:select-none={isResizing}
  >
    <!-- Chat main area (always on desktop, on mobile only when activeTab === 'chat') -->
    <div
      class={cn(
        "flex h-full flex-1 flex-col overflow-hidden",
        activeTab !== "chat" ? "hidden md:flex" : "flex",
      )}
    >
      <!-- Scrollable message timeline -->
      <div class="relative flex-1 min-h-0">
        <div
          bind:this={chatContainer}
          onscroll={handleScroll}
          class="h-full overflow-y-auto px-4 py-5 md:px-8"
        >
          <div class="mx-auto flex max-w-[760px] flex-col gap-4">
            <BootstrapLogCard lines={bootstrapLines} />
            <ChatTimeline {sessionId} {items} status={currentStatus} />
          </div>
        </div>

        <!-- Jump to bottom button -->
        {#if showScrollBottom}
          <button
            type="button"
            class="bg-card text-foreground border-border absolute bottom-4 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-[13px] font-medium shadow-md transition-all hover:bg-accent animate-in fade-in zoom-in-95 cursor-pointer"
            onclick={scrollToBottom}
          >
            <ArrowDownIcon class="size-3.5" />
            <span>{m.session_jump_to_latest()}</span>
          </button>
        {/if}
      </div>

      <!-- Chat input (Composer) -->
      <div class="border-t border-border px-4 py-3 md:px-8 shrink-0">
        <div class="mx-auto max-w-[760px]">
          <Composer
            {sessionId}
            {capabilities}
            node={sessionNode}
            status={currentStatus}
            permissionPolicy={session?.permission_policy ?? null}
            projectId={session?.project_id ?? ""}
            bind:resumeRequired
          />
        </div>
      </div>
    </div>

    <!-- Desktop right side panel with resizer (Diff or Terminal) -->
    {#if sidePanelMode !== null}
      <!-- Resizer divider -->
      <button
        type="button"
        aria-label="Resize panel"
        class={cn(
          "hidden md:flex w-1.5 cursor-col-resize hover:bg-signal/50 active:bg-signal transition-colors group select-none shrink-0 border-l border-border items-center justify-center -ml-[1px] z-10 outline-none focus-visible:ring-1 focus-visible:ring-signal bg-transparent p-0",
          isResizing && "bg-signal",
        )}
        onpointerdown={startResize}
        onkeydown={(e) => {
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            sidePanelWidth = Math.min(75, sidePanelWidth + 2);
          } else if (e.key === "ArrowRight") {
            e.preventDefault();
            sidePanelWidth = Math.max(20, sidePanelWidth - 2);
          }
        }}
      >
        <span
          class="w-0.5 h-8 bg-muted-foreground/30 rounded-full group-hover:bg-signal pointer-events-none"
        ></span>
      </button>

      <div
        class="border-border bg-card/40 hidden h-full flex-col overflow-hidden md:flex shrink-0"
        style="width: {sidePanelWidth}%;"
      >
        <!-- Side panel header -->
        <div class="flex items-center justify-between border-b border-border px-3 py-2">
          <div class="flex items-center gap-2 text-[13px] font-semibold">
            {#if sidePanelMode === "diff"}
              <FileCodeIcon class="size-4" />
              <span>{m.session_diff_panel_title()}</span>
            {:else}
              <SquareTerminalIcon class="size-4" />
              <span>{m.session_terminal_panel_title()}</span>
            {/if}
          </div>

          <div class="flex items-center gap-1">
            <button
              type="button"
              class={cn(
                "px-2 py-0.5 text-[13px] rounded",
                sidePanelMode === "diff"
                  ? "bg-secondary font-medium"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onclick={() => (sidePanelMode = "diff")}
            >
              Diff
            </button>
            <button
              type="button"
              class={cn(
                "px-2 py-0.5 text-[13px] rounded",
                sidePanelMode === "terminal"
                  ? "bg-secondary font-medium"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onclick={() => (sidePanelMode = "terminal")}
            >
              Terminal
            </button>
            <button
              type="button"
              class="text-muted-foreground hover:text-foreground ml-1 p-1"
              onclick={() => (sidePanelMode = null)}
              title={m.session_close_panel()}
            >
              <PanelRightCloseIcon class="size-4" />
            </button>
          </div>
        </div>

        <!-- Side panel content -->
        <div class="flex-1 overflow-y-auto p-3">
          {#if sidePanelMode === "diff"}
            <DiffPane {sessionId} {items} />
          {:else if sidePanelMode === "terminal"}
            <TerminalView {sessionId} />
          {/if}
        </div>
      </div>
    {/if}

    <!-- Mobile full-screen Diff / Terminal (when a tab is selected below md) -->
    {#if activeTab === "diff"}
      <div class="flex flex-1 flex-col overflow-y-auto p-3 md:hidden">
        <DiffPane {sessionId} {items} />
      </div>
    {:else if activeTab === "terminal"}
      <div class="flex flex-1 flex-col overflow-y-auto p-3 md:hidden">
        <TerminalView {sessionId} />
      </div>
    {/if}
  </div>
</div>
