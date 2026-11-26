<script lang="ts">
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { onDestroy, tick, untrack } from "svelte";
  import { fade, fly, slide } from "svelte/transition";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import ProvisioningLogCard from "#lib/components/ProvisioningLogCard.svelte";
  import ChatTimeline from "#lib/components/chat/ChatTimeline.svelte";
  import SubagentDetail from "#lib/components/chat/SubagentDetail.svelte";
  import TurnOutline from "#lib/components/chat/TurnOutline.svelte";
  import TurnRail from "#lib/components/chat/TurnRail.svelte";
  import { buildChatTurns, type ChatTurn } from "#lib/chat-turns.ts";
  import { groupTimelineItems } from "#lib/chat-groups.ts";
  import Composer from "#lib/components/chat/Composer.svelte";
  import SessionHeader from "#lib/components/session/SessionHeader.svelte";
  import DockHost from "#lib/components/dock/DockHost.svelte";
  import DockRail from "#lib/components/dock/DockRail.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Skeleton } from "#lib/components/ui/skeleton/index.ts";
  import { commandErrorMessage } from "#lib/api/client.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { forkScopeOf } from "#lib/fork-scope.ts";
  import type { SessionSummary } from "#lib/generated/bindings.ts";
  import type { DockLauncherId } from "#lib/dock/registry.ts";
  import { dock } from "#lib/stores/dock.svelte.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { motion } from "#lib/motion.svelte.ts";
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
  import { uiTraits } from "#lib/ui-traits.svelte.ts";
  import { registerOverlay } from "#lib/overlays.ts";
  import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import SkullIcon from "@lucide/svelte/icons/skull";

  const sessionId = $derived(page.params.id ?? "");
  /**
   * `node_seq` from a deep link (for example a global search hit). The page
   * loads older history until the matching row is rendered, then scrolls to it.
   */
  const deepLinkSeq = $derived.by(() => {
    const raw = page.url.searchParams.get("seq");
    if (raw === null) return null;
    const value = Number(raw);
    return Number.isSafeInteger(value) && value > 0 ? value : null;
  });
  /**
   * Session aggregate for the header.
   *
   * Deep links can point at a session outside the loaded sidebar pages (the
   * list is paginated), so fall back to `GET /api/v1/sessions/:id`.
   */
  let fallbackSession = $state<{ id: string; session: SessionSummary } | null>(null);
  const session = $derived(
    sync.sessions.find((entry) => entry.session_id === sessionId) ??
      (fallbackSession?.id === sessionId ? fallbackSession.session : null),
  );
  const sessionNode = $derived(
    session ? (sync.nodes.find((n) => n.node_id === session.node_id) ?? null) : null,
  );
  const projectName = $derived(
    session
      ? (sync.projects.find((project) => project.project_id === session.project_id)?.name ?? null)
      : null,
  );
  /** Current session as dock context; the workspace key is derived from node + path. */
  const dockContext = $derived(
    session === null
      ? null
      : { sessionId: session.session_id, nodeId: session.node_id, localPath: session.local_path },
  );
  const isDockOpen = $derived(dock.isOpen && dockContext !== null);
  /** Compact layouts show the dock as a full-screen layer; wide ones split the pane. */
  const dockAsOverlay = $derived(isDockOpen && uiTraits.overlayPanels);
  /** Highlight the active launcher only while the dock is visible. */
  const visibleLauncher = $derived(dock.isOpen ? dock.activeTool : null);

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
  const forkFromTitle = $derived(
    session?.parent_session_id
      ? (sync.sessions.find((entry) => entry.session_id === session.parent_session_id)?.title ??
          null)
      : null,
  );
  /** Last event copied from the parent (copied events keep the parent timestamps). */
  const forkAfterSeq = $derived.by(() => {
    if (!session?.parent_session_id) return null;
    for (let index = items.length - 1; index >= 0; index -= 1) {
      if (items[index].createdAt < session.created_at) return items[index].seq;
    }
    return null;
  });
  const capabilities = $derived(capabilitiesFromEvents(timeline.metadataEvents()));
  const canRevert = $derived(capabilities?.nativeFeatures.includes("revert") ?? false);
  // Prefer the latest usage derived from events, and fall back to the session list (projection) value.
  const usage = $derived(latestUsageFromEvents(timeline.events) ?? session?.usage ?? null);
  // Merge persistent events (ProvisioningLog synced after a turn completes) with the
  // ephemeral log the server streams from an ephemeral VM.
  const provisioningLines = $derived.by(() => {
    const persisted = provisioningLogLines(timeline.metadataEvents());
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
  /** Turns of the loaded timeline, the unit of the rail and the outline. */
  const turns = $derived(buildChatTurns(items));
  let turnsOpen = $state(false);
  /** Turn currently under the viewport; the rail measures it. */
  let activeTurnSeq = $state<number | null>(null);
  let loadedFor = $state<string | null>(null);
  let showScrollBottom = $state(false);
  /** Row flashed after a deep-link jump (cleared when the animation ends). */
  let highlightSeq = $state<number | null>(null);
  /** Plain guard: the effect re-runs as history arrives, but a jump happens once. */
  let handledDeepLink: string | null = null;
  let deepLinkToken = 0;
  let highlightTimer: ReturnType<typeof setTimeout> | null = null;
  /** Whether new output should pull the view along (true while reading the tail). */
  let followOutput = $state(true);
  let scrollFrame: number | null = null;
  /** Distance from the bottom that still counts as "reading the tail". */
  const FOLLOW_THRESHOLD_PX = 80;
  let lastUserItemKey: string | null = null;

  // Desktop resizable right pane
  let mainContainer = $state<HTMLDivElement | null>(null);
  let isResizing = $state(false);

  function handlePointerMove(e: PointerEvent) {
    if (!isResizing || !mainContainer) return;
    const rect = mainContainer.getBoundingClientRect();
    const pointerX = e.clientX;
    const rightPixels = rect.right - pointerX;
    const percentage = (rightPixels / rect.width) * 100;
    dock.setWidth(Math.round(percentage));
  }

  function handlePointerUp() {
    if (!isResizing) return;
    isResizing = false;
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", handlePointerUp);
    dock.commitWidth();
  }

  function startResize(event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    isResizing = true;
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
  }

  // The workspace owns the dock state; the session only supplies tool context.
  // Switching sessions inside one workspace must not reset the active tool or
  // open file previews.
  $effect(() => {
    dock.setContext(dockContext);
  });

  // Mobile layers close on the system back gesture instead of leaving the
  // session. The desktop dock is a split pane, so it is not in this stack.
  $effect(() => {
    if (!dockAsOverlay) return;
    return registerOverlay("session-dock", () => dock.collapse());
  });
  $effect(() => {
    if (!turnsOpen) return;
    return registerOverlay("turn-outline", () => (turnsOpen = false));
  });
  $effect(() => {
    if (selectedSubagentId === null) return;
    return registerOverlay("subagent-detail", () => (selectedSubagentId = null));
  });

  // Make the session a priority target for WS delivery, and fall back to REST
  // loading when the history has not been fetched yet.
  $effect(() => {
    const id = sessionId;
    if (!id) return;
    sync.setActiveSession(id);
    if (loadedFor !== id) {
      loadedFor = id;
      selectedSubagentId = null;
      turnsOpen = false;
      activeTurnSeq = null;
      sync.loadSessionEvents(id).catch(() => {
        /* Ignore: the WS replay will make it visible once it arrives. */
      });
    }
    return () => sync.setActiveSession(null);
  });

  // Deep links may point at a session outside the loaded sidebar pages.
  $effect(() => {
    const id = sessionId;
    if (!id) return;
    if (sync.sessions.some((entry) => entry.session_id === id)) return;
    if (fallbackSession?.id === id) return;
    let cancelled = false;
    void sync.connection.client
      .sessionDetail(id)
      .then((detail) => {
        if (!cancelled) fallbackSession = { id, session: detail.session };
      })
      .catch(() => {
        /* Keep the "not found" state of the header. */
      });
    return () => {
      cancelled = true;
    };
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

  // The force-stop confirmation closes on the system back gesture.
  $effect(() => {
    if (!killDialogOpen) return;
    return registerOverlay("session-kill", () => (killDialogOpen = false));
  });

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
  let composerRef = $state<ReturnType<typeof Composer> | null>(null);

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

  /** The rendered row that contains `seq` (groups expose a seq range). */
  function findSeqRow(container: HTMLElement, seq: number): HTMLElement | null {
    const rows = container.querySelectorAll<HTMLElement>("[data-seq]");
    for (const row of rows) {
      const start = Number(row.dataset.seq);
      const end = Number(row.dataset.seqEnd ?? row.dataset.seq);
      if (Number.isFinite(start) && Number.isFinite(end) && seq >= start && seq <= end) {
        return row;
      }
    }
    return null;
  }

  function clearHighlight(): void {
    if (highlightTimer !== null) {
      clearTimeout(highlightTimer);
      highlightTimer = null;
    }
    highlightSeq = null;
  }

  /**
   * Scroll to a deep-linked event and flash its row.
   *
   * Global search can hit messages far older than the loaded page. Older pages
   * are prepended until the row exists, or until the loaded history already
   * covers the target (the event may be folded into a group or not rendered).
   */
  async function focusDeepLink(id: string, seq: number): Promise<void> {
    const token = ++deepLinkToken;
    followOutput = false;
    clearHighlight();
    try {
      // A long session may need several pages; cap the work per navigation.
      for (let attempt = 0; attempt < 20; attempt += 1) {
        await tick();
        if (token !== deepLinkToken || sessionId !== id) return;
        const container = chatContainer;
        const row = container === null ? null : findSeqRow(container, seq);
        if (container !== null && row !== null) {
          const top =
            row.getBoundingClientRect().top -
            container.getBoundingClientRect().top +
            container.scrollTop -
            12;
          container.scrollTo({ top: Math.max(0, top), behavior: "auto" });
          highlightSeq = seq;
          highlightTimer = setTimeout(() => {
            if (token === deepLinkToken) highlightSeq = null;
            highlightTimer = null;
          }, 2600);
          return;
        }
        const history = sync.timelineFor(id);
        const oldest = history.events.at(0)?.node_seq ?? null;
        // The target is already inside the loaded range, so older pages cannot
        // add a row for it.
        if (oldest !== null && oldest <= seq) return;
        if (!history.hasOlder) return;
        if (!(await sync.loadOlderSessionEvents(id))) return;
      }
    } catch {
      /* Reported through the sync store's problem banner. */
    }
  }

  $effect(() => {
    const id = sessionId;
    const seq = deepLinkSeq;
    if (id === "" || seq === null) {
      handledDeepLink = null;
      clearHighlight();
      return;
    }
    // Wait for the newest page so the first attempt sees the timeline.
    if (!sync.timelineFor(id).historyLoaded) return;
    const key = `${id}:${seq}`;
    if (handledDeepLink === key) return;
    handledDeepLink = key;
    untrack(() => void focusDeepLink(id, seq));
    return () => {
      deepLinkToken += 1;
      clearHighlight();
    };
  });

  /** Scroll to the start of a turn (rail and outline navigation). */
  function jumpToTurn(turn: ChatTurn): void {
    const container = chatContainer;
    if (container === null) return;
    const anchor = container.querySelector<HTMLElement>(`[data-turn-seq="${turn.seq}"]`);
    if (anchor === null) return;
    // Landing on the latest turn keeps following the stream; anywhere else stops.
    followOutput = turn.seq === turns.at(-1)?.seq;
    const top =
      anchor.getBoundingClientRect().top -
      container.getBoundingClientRect().top +
      container.scrollTop -
      8;
    container.scrollTo({ top: Math.max(0, top), behavior: "smooth" });
  }

  let loadingOlder = $state(false);

  /** Keep the skeleton mounted for one crossfade after the first page arrives. */
  let showSkeleton = $state(untrack(() => timeline.historyLoading && items.length === 0));
  let skeletonTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const loading = timeline.historyLoading && items.length === 0;
    if (loading) {
      showSkeleton = true;
      if (skeletonTimer !== null) {
        clearTimeout(skeletonTimer);
        skeletonTimer = null;
      }
      return;
    }
    if (showSkeleton && skeletonTimer === null) {
      skeletonTimer = setTimeout(() => {
        showSkeleton = false;
        skeletonTimer = null;
      }, motion.duration(100));
    }
  });

  onDestroy(() => {
    if (skeletonTimer !== null) clearTimeout(skeletonTimer);
  });

  /**
   * Prepend the next older page of the session history and keep the reader's
   * position (the content above grows by the inserted height).
   */
  async function loadOlder() {
    const container = chatContainer;
    if (container === null || loadingOlder) return;
    const previousHeight = container.scrollHeight;
    const previousTop = container.scrollTop;
    loadingOlder = true;
    followOutput = false;
    try {
      const added = await sync.loadOlderSessionEvents(sessionId);
      if (added) {
        await tick();
        container.scrollTop = previousTop + (container.scrollHeight - previousHeight);
      }
    } catch {
      /* Reported through the sync store's problem banner. */
    } finally {
      loadingOlder = false;
    }
  }

  /** Mobile header: open the tool, or close the dock when it is already active. */
  function handleToggleTool(tool: DockLauncherId): void {
    dock.toggleTool(tool);
  }

  /** Revert the native conversation to the selected message boundary. */
  async function handleRevertItem(item: TimelineItem): Promise<void> {
    if (!sessionId) return;
    try {
      const result = await sync.controlSession(sessionId, {
        action: "revert",
        up_to_node_seq: item.seq,
        files: true,
      });
      if (result.success) {
        if (item.kind === "user" && item.text !== "") {
          composerRef?.restoreText(item.text);
        }
        toast.success(m.chat_action_reverted());
      } else {
        toast.error(commandErrorMessage(result.error) ?? m.chat_action_revert_failed());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  /** Terminate a command that is still running (a background dev server etc.). */
  async function handleStopTerminal(terminalId: string): Promise<void> {
    if (!sessionId) return;
    try {
      const result = await sync.controlSession(sessionId, {
        action: "kill_terminal",
        terminal_id: terminalId,
      });
      if (!result.success) {
        toast.error(commandErrorMessage(result.error) ?? m.chat_terminal_stop_failed());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
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
    activeTool={visibleLauncher}
    onToggleTool={handleToggleTool}
    {turnsOpen}
    onToggleTurns={() => (turnsOpen = !turnsOpen)}
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
    <!-- Chat column. In the compact shell the dock is an overlay, so the chat
         stays rendered (and dimmed) behind it; `inert` keeps it out of the tab
         order. -->
    <div class="flex h-full min-w-0 flex-1 flex-col overflow-hidden" inert={dockAsOverlay}>
      <!-- Scrollable message timeline -->
      <div class="relative min-h-0 flex-1">
        <div
          bind:this={chatContainer}
          onscroll={handleScroll}
          class="app-scroll-centered h-full overflow-y-auto pt-5 pr-8 pb-6 pl-4 md:px-8"
        >
          <div class="mx-auto flex max-w-[760px] flex-col gap-4">
            <ProvisioningLogCard lines={provisioningLines} />
            {#if timeline.hasOlder}
              <div class="flex justify-center">
                <Button
                  variant="outline"
                  size="sm"
                  class="gap-1.5 text-[12px]"
                  disabled={timeline.historyLoading}
                  onclick={loadOlder}
                >
                  {#if loadingOlder}
                    <LoaderCircleIcon class="size-3.5 animate-spin" />
                  {/if}
                  {m.chat_load_earlier()}
                </Button>
              </div>
            {/if}
            <div
              class="relative"
              class:min-h-[13rem]={timeline.historyLoading && items.length === 0}
            >
              {#if showSkeleton}
                <!-- The skeleton overlays the incoming timeline for one crossfade.
                     It is absolute so its own height never competes with the
                     real content; the wrapper reserves a stable placeholder
                     height while the first page is still loading. -->
                <div
                  class="pointer-events-none absolute inset-x-0 top-0 flex flex-col gap-5 py-1 transition-opacity duration-[100ms]"
                  class:opacity-0={!timeline.historyLoading || items.length > 0}
                  aria-busy="true"
                >
                  {#each [0, 1, 2] as row (row)}
                    <div class="flex gap-2.5">
                      <Skeleton class="size-5 shrink-0 rounded-md" />
                      <div class="flex min-w-0 flex-1 flex-col gap-2">
                        <Skeleton class="h-3.5 w-4/5" />
                        <Skeleton class="h-3.5 w-3/5" />
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}
              {#if !(timeline.historyLoading && items.length === 0)}
                <div in:fade|local={{ duration: motion.duration(100) }}>
                  <ChatTimeline
                    {sessionId}
                    {items}
                    status={currentStatus}
                    {forkScope}
                    forkFrom={session?.parent_session_id ?? null}
                    {forkFromTitle}
                    {forkAfterSeq}
                    {highlightSeq}
                    onfork={handleForkItem}
                    {canRevert}
                    onrevert={handleRevertItem}
                    onstopterminal={handleStopTerminal}
                    onsubagent={(childSessionId) => (selectedSubagentId = childSessionId)}
                  />
                </div>
              {/if}
            </div>
          </div>
        </div>

        {#if turns.length > 0}
          <TurnRail
            {turns}
            container={chatContainer}
            hasOlder={timeline.hasOlder}
            onloadearlier={() => void loadOlder()}
            onjump={jumpToTurn}
            onactive={(seq) => (activeTurnSeq = seq)}
          />
        {/if}

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

        {#if turnsOpen && turns.length > 0}
          <TurnOutline
            {turns}
            {sessionId}
            activeSeq={activeTurnSeq}
            onjump={jumpToTurn}
            onjumpseq={(seq) => void focusDeepLink(sessionId, seq)}
            onclose={() => (turnsOpen = false)}
          />
        {/if}
      </div>

      <!-- Chat input (Composer) -->
      <div class="border-border shrink-0 border-t px-4 py-2 md:px-8">
        <div class="mx-auto max-w-[760px]">
          {#key sessionId}
            <Composer
              bind:this={composerRef}
              {sessionId}
              {capabilities}
              node={sessionNode}
              status={currentStatus}
              permissionPolicy={session?.permission_policy ?? null}
              projectId={session?.project_id ?? ""}
              agentId={session?.agent_id ?? ""}
              bind:resumeRequired
            />
          {/key}
        </div>
      </div>
    </div>

    <!-- Wide workspace dock -->
    {#if isDockOpen && dockContext !== null && !uiTraits.overlayPanels}
      {@const ctx = dockContext}
      <button
        type="button"
        aria-label={m.dock_resize()}
        in:fade|local={{ duration: motion.duration(120) }}
        out:fade|local={{ duration: motion.duration(120) }}
        class={cn(
          "group z-10 -ml-px flex w-1.5 cursor-col-resize shrink-0 items-center justify-center border-l border-border bg-transparent p-0 outline-none transition-colors select-none hover:bg-signal/50 focus-visible:ring-1 focus-visible:ring-signal active:bg-signal",
          isResizing && "bg-signal",
        )}
        onpointerdown={startResize}
        onkeydown={(e) => {
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            dock.setWidth(dock.width + 2);
            dock.commitWidth();
          } else if (e.key === "ArrowRight") {
            e.preventDefault();
            dock.setWidth(dock.width - 2);
            dock.commitWidth();
          }
        }}
      >
        <span
          class="bg-muted-foreground/30 group-hover:bg-signal h-8 w-0.5 rounded-full pointer-events-none"
        ></span>
      </button>

      <div
        in:slide|local={{ axis: "x", duration: motion.duration(180) }}
        out:slide|local={{ axis: "x", duration: motion.duration(180) }}
        class="flex h-full shrink-0 flex-col overflow-hidden"
        style="width: clamp(22rem, {dock.width}%, 75%);"
      >
        <DockHost {ctx} {items} oncollapse={() => dock.collapse()} />
      </div>
    {/if}

    <!-- Wide tool rail -->
    {#if !uiTraits.overlayPanels && !layoutState.rightRailCollapsed && session !== null}
      <div class="flex h-full">
        <DockRail
          active={visibleLauncher}
          onselect={(id) => dock.toggleTool(id)}
          oncollapse={() => layoutState.setRightRailCollapsed(true)}
        />
      </div>
    {/if}

    <!-- Compact dock overlay. The inset panel and the dimmed, blurred backdrop
         make it obvious that this is a layer above the conversation. -->
    {#if isDockOpen && dockContext !== null && uiTraits.overlayPanels}
      {@const ctx = dockContext}
      <div class="absolute inset-0 z-20">
        <button
          type="button"
          tabindex="-1"
          aria-label={m.session_close_panel()}
          transition:fade|local={{ duration: motion.duration(150) }}
          class="bg-background/60 absolute inset-0 cursor-default backdrop-blur-[2px]"
          onclick={() => dock.collapse()}
        ></button>
        <div
          transition:fly|local={{ y: 24, duration: motion.duration(180) }}
          class="bg-card absolute inset-0 flex flex-col overflow-hidden"
        >
          <DockHost {ctx} {items} mobile oncollapse={() => dock.collapse()} />
        </div>
      </div>
    {/if}
  </div>
</div>
