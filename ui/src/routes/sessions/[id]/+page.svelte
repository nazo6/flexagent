<script lang="ts">
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import ProvisioningLogCard from "#lib/components/ProvisioningLogCard.svelte";
  import ChatTimeline from "#lib/components/chat/ChatTimeline.svelte";
  import SubagentDetail from "#lib/components/chat/SubagentDetail.svelte";
  import { groupTimelineItems } from "#lib/chat-groups.ts";
  import Composer from "#lib/components/chat/Composer.svelte";
  import SessionHeader from "#lib/components/session/SessionHeader.svelte";
  import SessionPanelHost from "#lib/components/session/SessionPanelHost.svelte";
  import SessionToolRail from "#lib/components/session/SessionToolRail.svelte";
  import SessionToolsSheet from "#lib/components/session/SessionToolsSheet.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { commandErrorMessage } from "#lib/api/client.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { forkScopeOf } from "#lib/fork-scope.ts";
  import type { SessionPanelId } from "#lib/session-panels.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import {
    provisioningLogLines,
    buildTimelineItems,
    capabilitiesFromEvents,
    latestStatusFromEvents,
    latestUsageFromEvents,
    type TimelineItem,
  } from "#lib/sync/reducer.ts";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import ArchiveIcon from "@lucide/svelte/icons/archive";
  import { layoutState } from "#lib/stores/layout.svelte.ts";
  import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
  import SkullIcon from "@lucide/svelte/icons/skull";

  const sessionId = $derived(page.params.id ?? "");
  const session = $derived(sync.sessions.find((entry) => entry.session_id === sessionId) ?? null);
  const sessionNode = $derived(
    session ? (sync.nodes.find((n) => n.node_id === session.node_id) ?? null) : null,
  );
  const projectName = $derived(
    session
      ? (sync.projects.find((project) => project.project_id === session.project_id)?.name ?? null)
      : null,
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
      subagentByMessage: timeline.subagentByMessage,
      subagentByThought: timeline.subagentByThought,
      toolProgress: timeline.toolProgress,
      terminalDeltas: timeline.terminalDeltas,
      thoughtTimes: timeline.thoughtTimes,
      streamKeys: timeline.streamKeys,
      pendingPrompts: sync.pendingPrompts,
    }),
  );
  const capabilities = $derived(capabilitiesFromEvents(timeline.events));
  // Prefer the latest usage derived from events, and fall back to the session list (projection) value.
  const usage = $derived(latestUsageFromEvents(timeline.events) ?? session?.usage ?? null);
  // Merge persistent events (ProvisioningLog synced after a turn completes) with the
  // ephemeral log the server streams from an ephemeral VM.
  const provisioningLines = $derived.by(() => {
    const persisted = provisioningLogLines(timeline.events);
    const live = sync.provisioningLinesFor(sessionId);
    return live.length === 0 ? persisted : [...new Set([...persisted, ...live])];
  });

  let selectedSubagentId = $state<string | null>(null);
  const selectedSubagent = $derived.by(() => {
    if (!selectedSubagentId) return null;
    const group = groupTimelineItems(items).find(
      (candidate) =>
        candidate.kind === "subagent" && candidate.subagent.childSessionId === selectedSubagentId,
    );
    return group?.kind === "subagent" ? group : null;
  });

  let chatContainer = $state<HTMLDivElement | null>(null);
  let loadedFor = $state<string | null>(null);
  let showScrollBottom = $state(false);
  /** Whether new output should pull the view along (true while reading the tail). */
  let followOutput = $state(true);
  let scrollFrame: number | null = null;
  /** Distance from the bottom that still counts as "reading the tail". */
  const FOLLOW_THRESHOLD_PX = 80;
  let lastUserItemKey: string | null = null;

  // Right tool rail / mobile tool sheet
  let activePanel = $state<SessionPanelId | null>(null);
  let toolsSheetOpen = $state(false);

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
      selectedSubagentId = null;
      activePanel = null;
      const target = sync.timelineFor(id);
      if (target.events.length === 0) {
        sync.loadSessionEvents(id).catch(() => {
          /* Ignore: the WS replay will make it visible once it arrives. */
        });
      }
    }
    return () => sync.setActiveSession(null);
  });

  // Follow the tail while the reader is there. Coalesced into one scroll per
  // frame: streamed deltas rebuild the timeline several times per second.
  function scheduleScrollToBottom(): void {
    if (scrollFrame !== null) return;
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = null;
      if (chatContainer === null || !followOutput) return;
      chatContainer.scrollTop = chatContainer.scrollHeight;
    });
  }

  $effect(() => () => {
    if (scrollFrame !== null) {
      cancelAnimationFrame(scrollFrame);
      scrollFrame = null;
    }
  });

  $effect(() => {
    // Reading both subscribes to the container being mounted and to content
    // growth (new items as well as streamed text inside an existing item).
    const container = chatContainer;
    const currentItems = items;
    if (container === null || currentItems.length === 0) return;
    const userItemKey = lastUserKeyOf(currentItems);
    if (userItemKey !== lastUserItemKey) {
      // A message was just sent: jump back to the conversation tail.
      lastUserItemKey = userItemKey;
      followOutput = true;
    }
    if (followOutput) scheduleScrollToBottom();
  });

  function lastUserKeyOf(currentItems: TimelineItem[]): string | null {
    for (let index = currentItems.length - 1; index >= 0; index -= 1) {
      const item = currentItems[index];
      if (item.kind === "user") return item.key;
    }
    return null;
  }

  function handleScroll() {
    if (!chatContainer) return;
    const distanceToBottom =
      chatContainer.scrollHeight - chatContainer.scrollTop - chatContainer.clientHeight;
    showScrollBottom = distanceToBottom > 150;
    followOutput = distanceToBottom < FOLLOW_THRESHOLD_PX;
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

  /** Fork capability of the open session. */
  const forkScope = $derived(forkScopeOf(session));

  function scrollToBottom() {
    if (!chatContainer) return;
    followOutput = true;
    chatContainer.scrollTo({
      top: chatContainer.scrollHeight,
      behavior: "smooth",
    });
    showScrollBottom = false;
  }

  function selectPanel(id: SessionPanelId): void {
    activePanel = activePanel === id ? null : id;
  }

  function openPanel(id: SessionPanelId): void {
    activePanel = id;
  }

  function handleForkItem(item: TimelineItem): void {
    if (!sessionId || !session) return;
    const params = new URLSearchParams();
    params.set("fork_session", sessionId);
    params.set("project", session.project_id);
    params.set("node", session.node_id);
    params.set("path", session.local_path);
    if (session.agent_id) params.set("agent", session.agent_id);
    // A message-scope agent can fork at a boundary. `last` agents must fork at
    // the current end, so their request intentionally omits the boundary.
    if (forkScope === "message") params.set("fork_upto", String(item.seq));
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
      const result = await sync.controlSession(sessionId, { action: "dispose" });
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
  <SessionHeader
    {session}
    node={sessionNode}
    status={currentStatus}
    {usage}
    {projectName}
    {canResume}
    onResume={() => void handleResumeSession()}
    canNewSessionHere={sessionNode !== null && !sessionNode.is_ephemeral}
    onNewSessionHere={handleNewSessionHere}
    onForceStop={() => (killDialogOpen = true)}
    onOpenTools={() => (toolsSheetOpen = true)}
    onDeleted={() => void goto("/")}
  />

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
    class="relative flex flex-1 overflow-hidden"
    class:select-none={isResizing}
  >
    <!-- Chat column -->
    <div
      class={cn(
        "flex h-full min-w-0 flex-1 flex-col overflow-hidden",
        activePanel !== null && session !== null && "hidden lg:flex",
      )}
    >
      <!-- Scrollable message timeline -->
      <div class="relative min-h-0 flex-1">
        <div
          bind:this={chatContainer}
          onscroll={handleScroll}
          class="app-scroll-centered h-full overflow-y-auto px-4 py-5 md:px-8"
        >
          <div class="mx-auto flex max-w-[760px] flex-col gap-4">
            <ProvisioningLogCard lines={provisioningLines} />
            <ChatTimeline
              {sessionId}
              {items}
              status={currentStatus}
              {forkScope}
              onfork={handleForkItem}
              onsubagent={(childSessionId) => (selectedSubagentId = childSessionId)}
            />
          </div>
        </div>

        {#if selectedSubagent}
          <SubagentDetail
            {sessionId}
            subagent={selectedSubagent.subagent}
            items={selectedSubagent.items}
            status={currentStatus}
            onclose={() => (selectedSubagentId = null)}
          />
        {/if}

        <!-- Jump to bottom button -->
        {#if showScrollBottom}
          <button
            type="button"
            class="bg-card text-foreground border-border hover:bg-accent animate-in fade-in zoom-in-95 absolute bottom-4 left-1/2 z-10 flex -translate-x-1/2 cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1.5 text-[13px] font-medium shadow-md transition-all"
            onclick={scrollToBottom}
          >
            <ArrowDownIcon class="size-3.5" />
            <span>{m.session_jump_to_latest()}</span>
          </button>
        {/if}
      </div>

      <!-- Chat input (Composer) -->
      <div class="border-border shrink-0 border-t px-4 py-3 md:px-8">
        <div class="mx-auto max-w-[760px]">
          <Composer
            {sessionId}
            {capabilities}
            node={sessionNode}
            status={currentStatus}
            permissionPolicy={session?.permission_policy ?? null}
            projectId={session?.project_id ?? ""}
            agentId={session?.agent_id ?? ""}
            bind:resumeRequired
          />
        </div>
      </div>
    </div>

    <!-- Desktop split panel -->
    {#if activePanel !== null && session !== null}
      <button
        type="button"
        aria-label="Resize panel"
        class={cn(
          "group z-10 -ml-px hidden w-1.5 cursor-col-resize shrink-0 items-center justify-center border-l border-border bg-transparent p-0 outline-none transition-colors select-none hover:bg-signal/50 focus-visible:ring-1 focus-visible:ring-signal active:bg-signal lg:flex",
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
          class="bg-muted-foreground/30 group-hover:bg-signal h-8 w-0.5 rounded-full pointer-events-none"
        ></span>
      </button>

      <div
        class="hidden h-full shrink-0 flex-col overflow-hidden lg:flex"
        style="width: {sidePanelWidth}%;"
      >
        <SessionPanelHost
          panel={activePanel}
          {sessionId}
          {items}
          onclose={() => (activePanel = null)}
        />
      </div>
    {/if}

    <!-- Desktop tool rail -->
    {#if !layoutState.rightRailCollapsed && session !== null}
      <div class="hidden h-full lg:flex">
        <SessionToolRail
          active={activePanel}
          onselect={selectPanel}
          oncollapse={() => layoutState.setRightRailCollapsed(true)}
        />
      </div>
    {/if}

    <!-- Mobile full-screen panel -->
    {#if activePanel !== null && session !== null}
      <div class="flex flex-1 flex-col overflow-hidden lg:hidden">
        <SessionPanelHost
          panel={activePanel}
          {sessionId}
          {items}
          mobile
          onclose={() => (activePanel = null)}
        />
      </div>
    {/if}
  </div>

  <SessionToolsSheet bind:open={toolsSheetOpen} active={activePanel} onselect={openPanel} />
</div>
