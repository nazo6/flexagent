import { createEffect, createMemo, createSignal, onSettled, Show } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import ProvisioningLogCard from "#lib/components/ProvisioningLogCard.tsx";
import ChatTimeline from "#lib/components/chat/ChatTimeline.tsx";
import Composer, { type ComposerApi } from "#lib/components/chat/Composer.tsx";
import SubagentDetail from "#lib/components/chat/SubagentDetail.tsx";
import SessionHeader from "#lib/components/session/SessionHeader.tsx";
import DockHost from "#lib/components/dock/DockHost.tsx";
import DockRail from "#lib/components/dock/DockRail.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { commandErrorMessage } from "#lib/api/client.ts";
import { m } from "#lib/paraglide/messages.js";
import { forkScopeOf } from "#lib/fork-scope.ts";
import type { SessionSummary } from "#lib/generated/bindings.ts";
import type { DockLauncherId } from "#lib/dock/registry.ts";
import { dock, type DockContext } from "#lib/stores/dock.ts";
import { sync } from "#lib/stores/app.ts";
import { layoutState } from "#lib/stores/layout.ts";
import { groupTimelineItems } from "#lib/chat-groups.ts";
import {
  provisioningLogLines,
  buildTimelineItems,
  capabilitiesFromEvents,
  latestStatusFromEvents,
  latestUsageFromEvents,
  type TimelineItem,
} from "#lib/sync/reducer.ts";
import { cn } from "#lib/utils.ts";
import IconArchive from "~icons/lucide/archive";
import IconArrowDown from "~icons/lucide/arrow-down";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import IconSkull from "~icons/lucide/skull";

/** Distance from the bottom that still counts as "reading the tail". */
const FOLLOW_THRESHOLD_PX = 80;
/**
 * The side panel has a desktop slot (split pane) and a mobile slot
 * (full-screen). Only the slot matching the viewport is rendered, because
 * mounting both started two terminals — and therefore two PTYs — at once.
 * `lg` is 64rem in Tailwind.
 */
const DESKTOP_LAYOUT_QUERY = "(min-width: 64rem)";

function lastUserKeyOf(currentItems: TimelineItem[]): string | null {
  for (let index = currentItems.length - 1; index >= 0; index -= 1) {
    const item = currentItems[index];
    if (item.kind === "user") return item.key;
  }
  return null;
}

/** Session screen: header, conversation timeline, composer and the dock. */
export default function SessionPage() {
  const params = useParams<{ id: string }>();
  const navigate = useNavigate();

  const sessionId = createMemo(() => params.id ?? "");
  /**
   * Session aggregate for the header.
   *
   * Deep links can point at a session outside the loaded sidebar pages (the
   * list is paginated), so fall back to `GET /api/v1/sessions/:id`.
   */
  const [fallbackSession, setFallbackSession] = createSignal<{
    id: string;
    session: SessionSummary;
  } | null>(null);
  const session = createMemo(
    () =>
      sync.sessions.find((entry) => entry.session_id === sessionId()) ??
      (fallbackSession()?.id === sessionId() ? (fallbackSession()?.session ?? null) : null),
  );
  const sessionNode = createMemo(() => {
    const current = session();
    return current ? (sync.nodes.find((n) => n.node_id === current.node_id) ?? null) : null;
  });
  const projectName = createMemo(() => {
    const current = session();
    return current
      ? (sync.projects.find((project) => project.project_id === current.project_id)?.name ?? null)
      : null;
  });
  /** Current session as dock context; the workspace key is derived from node + path. */
  const dockContext = createMemo<DockContext | null>(() => {
    const current = session();
    return current === null
      ? null
      : {
          sessionId: current.session_id,
          nodeId: current.node_id,
          localPath: current.local_path,
        };
  });
  const isDockOpen = () => dock.isOpen && dockContext() !== null;
  /** Highlight the active launcher only while the dock is visible. */
  const visibleLauncher = (): DockLauncherId | null => (dock.isOpen ? dock.activeTool : null);

  const timeline = createMemo(() => sync.timelineFor(sessionId()));
  const currentStatus = () =>
    latestStatusFromEvents(timeline().events)?.status ?? session()?.status ?? null;
  const items = createMemo(() => {
    const source = timeline();
    return buildTimelineItems({
      sessionId: sessionId(),
      events: source.events,
      messageDeltas: source.messageDeltas,
      thoughtDeltas: source.thoughtDeltas,
      subagentByMessage: source.subagentByMessage,
      subagentByThought: source.subagentByThought,
      toolProgress: source.toolProgress,
      terminalDeltas: source.terminalDeltas,
      thoughtTimes: source.thoughtTimes,
      streamKeys: source.streamKeys,
      pendingPrompts: sync.pendingPrompts,
    });
  });
  const forkFromTitle = () => {
    const parentId = session()?.parent_session_id ?? null;
    return parentId === null
      ? null
      : (sync.sessions.find((entry) => entry.session_id === parentId)?.title ?? null);
  };
  /** Last event copied from the parent (copied events keep the parent timestamps). */
  const forkAfterSeq = createMemo(() => {
    const current = session();
    if (!current?.parent_session_id) return null;
    const list = items();
    for (let index = list.length - 1; index >= 0; index -= 1) {
      if (list[index].createdAt < current.created_at) return list[index].seq;
    }
    return null;
  });
  const capabilities = () => capabilitiesFromEvents(timeline().metadataEvents());
  const canRevert = () => capabilities()?.nativeFeatures.includes("revert") ?? false;
  // Prefer the latest usage derived from events, and fall back to the session list (projection) value.
  const usage = () => latestUsageFromEvents(timeline().events) ?? session()?.usage ?? null;
  // Merge persistent events (ProvisioningLog synced after a turn completes) with the
  // ephemeral log the server streams from an ephemeral VM.
  const provisioningLines = createMemo(() => {
    const persisted = provisioningLogLines(timeline().metadataEvents());
    const live = sync.provisioningLinesFor(sessionId());
    return live.length === 0 ? persisted : [...new Set([...persisted, ...live])];
  });

  const [selectedSubagentId, setSelectedSubagentId] = createSignal<string | null>(null);
  const selectedSubagent = createMemo(() => {
    const target = selectedSubagentId();
    if (target === null || target === "") return null;
    const group = groupTimelineItems(items()).find(
      (candidate) => candidate.kind === "subagent" && candidate.subagent.childSessionId === target,
    );
    return group?.kind === "subagent" ? group : null;
  });

  const [chatContainer, setChatContainer] = createSignal<HTMLDivElement | null>(null);
  let loadedFor: string | null = null;
  const [showScrollBottom, setShowScrollBottom] = createSignal(false);
  /** Whether new output should pull the view along (true while reading the tail). */
  const [followOutput, setFollowOutput] = createSignal(true);
  let scrollFrame: number | null = null;
  let lastUserItemKey: string | null = null;

  // Desktop resizable right pane
  const [mainContainer, setMainContainer] = createSignal<HTMLDivElement | null>(null);
  const [isResizing, setIsResizing] = createSignal(false);

  const [desktopLayout, setDesktopLayout] = createSignal(
    typeof window === "undefined" || typeof window.matchMedia !== "function"
      ? true
      : window.matchMedia(DESKTOP_LAYOUT_QUERY).matches,
  );

  onSettled(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(DESKTOP_LAYOUT_QUERY);
    setDesktopLayout(query.matches);
    const onChange = (event: MediaQueryListEvent) => setDesktopLayout(event.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  });

  function handlePointerMove(e: PointerEvent): void {
    const container = mainContainer();
    if (!isResizing() || !container) return;
    const rect = container.getBoundingClientRect();
    const pointerX = e.clientX;
    const rightPixels = rect.right - pointerX;
    const percentage = (rightPixels / rect.width) * 100;
    dock.setWidth(Math.round(percentage));
  }

  function handlePointerUp(): void {
    if (!isResizing()) return;
    setIsResizing(false);
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", handlePointerUp);
    dock.commitWidth();
  }

  function startResize(event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    setIsResizing(true);
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
  }

  // The workspace owns the dock state; the session only supplies tool context.
  // Switching sessions inside one workspace must not reset the active tool or
  // open file previews.
  createEffect(
    () => dockContext(),
    (context) => {
      dock.setContext(context);
    },
  );

  // Make the session a priority target for WS delivery, and fall back to REST
  // loading when the history has not been fetched yet.
  createEffect(
    () => sessionId(),
    (id) => {
      if (!id) return;
      sync.setActiveSession(id);
      if (loadedFor !== id) {
        loadedFor = id;
        setSelectedSubagentId(null);
        sync.loadSessionEvents(id).catch(() => {
          /* Ignore: the WS replay will make it visible once it arrives. */
        });
      }
      return () => sync.setActiveSession(null);
    },
  );

  // Deep links may point at a session outside the loaded sidebar pages.
  createEffect(
    () => ({
      id: sessionId(),
      listed: sync.sessions.some((entry) => entry.session_id === sessionId()),
    }),
    ({ id, listed }) => {
      if (!id) return;
      if (listed) return;
      if (fallbackSession()?.id === id) return;
      let cancelled = false;
      void sync.connection.client
        .sessionDetail(id)
        .then((detail) => {
          if (!cancelled) setFallbackSession({ id, session: detail.session });
        })
        .catch(() => {
          /* Keep the "not found" state of the header. */
        });
      return () => {
        cancelled = true;
      };
    },
  );

  // Follow the tail while the reader is there. Coalesced into one scroll per
  // frame: streamed deltas rebuild the timeline several times per second.
  function scheduleScrollToBottom(): void {
    if (scrollFrame !== null) return;
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = null;
      const container = chatContainer();
      if (container === null || !followOutput()) return;
      container.scrollTop = container.scrollHeight;
    });
  }

  createEffect(
    () => undefined,
    () => () => {
      if (scrollFrame !== null) {
        cancelAnimationFrame(scrollFrame);
        scrollFrame = null;
      }
    },
  );

  createEffect(
    () => ({
      container: chatContainer(),
      items: items(),
      follow: followOutput(),
      userKey: lastUserKeyOf(items()),
    }),
    ({ container, items: currentItems, follow, userKey }) => {
      // Reading the container and the item list subscribes to the container
      // being mounted and to content growth (new items as well as streamed text
      // inside an existing item).
      if (container === null || currentItems.length === 0) return;
      if (userKey !== lastUserItemKey) {
        // A message was just sent: jump back to the conversation tail.
        lastUserItemKey = userKey;
        setFollowOutput(true);
      }
      if (follow) scheduleScrollToBottom();
    },
  );

  function handleScroll(): void {
    const container = chatContainer();
    if (!container) return;
    const distanceToBottom =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    setShowScrollBottom(distanceToBottom > 150);
    setFollowOutput(distanceToBottom < FOLLOW_THRESHOLD_PX);
  }

  const [killDialogOpen, setKillDialogOpen] = createSignal(false);
  const [killing, setKilling] = createSignal(false);

  // Execution state of Resume (restarting a stopped session)
  const [resuming, setResuming] = createSignal(false);

  /**
   * Whether native restore is unsupported (`RESUME_REQUIRED`) was detected.
   *
   * Normally a stopped session is resumed automatically on send, so the Resume
   * button is not shown. It is only shown when sending failed with
   * `RESUME_REQUIRED` (i.e. resuming via history replay is required). Shared
   * with the composer.
   */
  const [resumeRequired, setResumeRequired] = createSignal(false);
  let composerApi: ComposerApi | undefined;

  /** Show Resume only for sessions where unsupported native restore was detected. */
  const canResume = () => resumeRequired() && session() !== null;

  /** Fork capability of the open session. */
  const forkScope = () => forkScopeOf(session());

  function scrollToBottom(): void {
    const container = chatContainer();
    if (!container) return;
    setFollowOutput(true);
    container.scrollTo({
      top: container.scrollHeight,
      behavior: "smooth",
    });
    setShowScrollBottom(false);
  }

  const [loadingOlder, setLoadingOlder] = createSignal(false);

  /**
   * Prepend the next older page of the session history and keep the reader's
   * position (the content above grows by the inserted height).
   */
  async function loadOlder(): Promise<void> {
    const container = chatContainer();
    if (container === null || loadingOlder()) return;
    const previousHeight = container.scrollHeight;
    const previousTop = container.scrollTop;
    setLoadingOlder(true);
    setFollowOutput(false);
    try {
      const added = await sync.loadOlderSessionEvents(sessionId());
      if (added) {
        // The list has already been rendered by the time the promise settles;
        // keep the reader anchored to the same content.
        container.scrollTop = previousTop + (container.scrollHeight - previousHeight);
      }
    } catch {
      /* Reported through the sync store's problem banner. */
    } finally {
      setLoadingOlder(false);
    }
  }

  /** Mobile header: open the tool, or close the dock when it is already active. */
  function handleToggleTool(tool: DockLauncherId): void {
    dock.toggleTool(tool);
  }

  /** Revert the native conversation to the selected message boundary. */
  async function handleRevertItem(item: TimelineItem): Promise<void> {
    if (!sessionId()) return;
    try {
      const result = await sync.controlSession(sessionId(), {
        action: "revert",
        up_to_node_seq: item.seq,
        files: true,
      });
      if (result.success) {
        if (item.kind === "user" && item.text !== "") {
          composerApi?.restoreText(item.text);
        }
        toast.success(m.chat_action_reverted());
      } else {
        toast.error(commandErrorMessage(result.error) ?? m.chat_action_revert_failed());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  function handleForkItem(item: TimelineItem): void {
    const current = session();
    if (!sessionId() || !current) return;
    const params = new URLSearchParams();
    params.set("fork_session", sessionId());
    params.set("project", current.project_id);
    params.set("node", current.node_id);
    params.set("path", current.local_path);
    if (current.agent_id) params.set("agent", current.agent_id);
    // A message-scope agent can fork at a boundary. `last` agents must fork at
    // the current end, so their request intentionally omits the boundary.
    if (forkScope() === "message") params.set("fork_upto", String(item.seq));
    navigate(`/?${params.toString()}`);
  }

  /** Open a new session on the same node and working directory as this session. */
  function handleNewSessionHere(): void {
    const current = session();
    if (!current) return;
    const params = new URLSearchParams();
    params.set("project", current.project_id);
    params.set("node", current.node_id);
    params.set("path", current.local_path);
    if (current.agent_id) params.set("agent", current.agent_id);
    navigate(`/?${params.toString()}`);
  }

  /** Resume a stopped session via history replay (`POST /sessions/:id/resume`). */
  async function handleResumeSession(): Promise<void> {
    if (!sessionId() || resuming()) return;
    setResuming(true);
    try {
      const result = await sync.resumeSession(sessionId());
      setResumeRequired(false);
      if (result.context_restored) {
        toast.success(m.session_resume_success_context());
      } else {
        toast.success(m.session_resume_success_replay());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setResuming(false);
    }
  }

  async function handleKillSession(): Promise<void> {
    if (!sessionId() || killing()) return;
    setKilling(true);
    try {
      const result = await sync.controlSession(sessionId(), { action: "dispose" });
      if (result.success) {
        toast.success(m.session_kill_success());
        setKillDialogOpen(false);
        await sync.refreshSessions();
      } else {
        toast.error(commandErrorMessage(result.error) ?? m.session_kill_failed());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setKilling(false);
    }
  }

  /** Unarchive the session (from the archived banner). */
  async function handleUnarchive(): Promise<void> {
    if (!sessionId()) return;
    try {
      await sync.archiveSession(sessionId(), false);
      toast.success(m.session_unarchived());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  createEffect(
    () => session()?.title,
    (title) => {
      if (typeof document === "undefined") return;
      document.title = `${title ?? m.session_title_fallback()} - FlexAgent`;
    },
  );

  return (
    <div class="bg-background flex h-full w-full flex-col overflow-hidden">
      <SessionHeader
        session={session()}
        node={sessionNode()}
        status={currentStatus()}
        usage={usage()}
        projectName={projectName()}
        canResume={canResume()}
        onResume={() => void handleResumeSession()}
        canNewSessionHere={sessionNode() !== null && !sessionNode()?.is_ephemeral}
        onNewSessionHere={handleNewSessionHere}
        onForceStop={() => setKillDialogOpen(true)}
        activeTool={visibleLauncher()}
        onToggleTool={handleToggleTool}
        onDeleted={() => navigate("/")}
      />

      {/* Archived banner */}
      <Show when={(session()?.archived_at ?? null) !== null}>
        <div class="border-warn/30 bg-warn-soft/50 flex shrink-0 items-center justify-between gap-2 border-b px-4 py-2 text-[13px]">
          <div class="text-warn flex items-center gap-2">
            <IconArchive class="size-4" />
            <span>{m.session_archived_banner()}</span>
          </div>
          <Button
            variant="outline"
            size="sm"
            class="h-7 shrink-0 gap-1 text-[11px]"
            onClick={() => void handleUnarchive()}
          >
            {m.session_unarchive()}
          </Button>
        </div>
      </Show>

      {/* Kill confirmation modal */}
      <AlertDialog.Root open={killDialogOpen()} onOpenChange={setKillDialogOpen}>
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title class="text-destructive flex items-center gap-2">
              <IconSkull class="size-5" />
              {m.session_kill_confirm_title()}
            </AlertDialog.Title>
            <AlertDialog.Description class="text-sm">
              {m.session_kill_confirm_description_pre()}
              <code>{sessionId()}</code>
              {m.session_kill_confirm_description_post()}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel disabled={killing()}>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action
              class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
              disabled={killing()}
              onClick={() => void handleKillSession()}
            >
              {killing() ? m.session_killing() : m.session_kill_action()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>

      {/* Error display when the session does not exist */}
      <Show when={session() === null}>
        <div class="text-muted-foreground p-4 text-center text-sm">{m.session_not_found()}</div>
      </Show>

      {/* Main content area */}
      <div
        ref={setMainContainer}
        class={cn("relative flex flex-1 overflow-hidden", isResizing() && "select-none")}
      >
        {/* Chat column. On mobile the dock is an overlay, so the chat stays
            rendered (and dimmed) behind it; `inert` keeps it out of the tab order. */}
        <div
          class="flex h-full min-w-0 flex-1 flex-col overflow-hidden"
          inert={isDockOpen() && !desktopLayout()}
        >
          {/* Scrollable message timeline */}
          <div class="relative min-h-0 flex-1">
            <div
              ref={setChatContainer}
              onScroll={handleScroll}
              class="app-scroll-centered h-full overflow-y-auto px-4 pt-5 pb-6 md:px-8"
            >
              <div class="mx-auto flex max-w-[760px] flex-col gap-4">
                <ProvisioningLogCard lines={provisioningLines()} />
                <Show when={timeline().hasOlder}>
                  <div class="flex justify-center">
                    <Button
                      variant="outline"
                      size="sm"
                      class="gap-1.5 text-[12px]"
                      disabled={timeline().historyLoading}
                      onClick={() => void loadOlder()}
                    >
                      <Show when={loadingOlder()}>
                        <IconLoaderCircle class="size-3.5 animate-spin" />
                      </Show>
                      {m.chat_load_earlier()}
                    </Button>
                  </div>
                </Show>
                <ChatTimeline
                  sessionId={sessionId()}
                  items={items()}
                  status={currentStatus()}
                  forkScope={forkScope()}
                  forkFrom={session()?.parent_session_id ?? null}
                  forkFromTitle={forkFromTitle()}
                  forkAfterSeq={forkAfterSeq()}
                  onfork={handleForkItem}
                  canRevert={canRevert()}
                  onrevert={(item) => void handleRevertItem(item)}
                  onsubagent={(childSessionId) => setSelectedSubagentId(childSessionId)}
                />
              </div>
            </div>

            <Show when={selectedSubagent()}>
              {(subagent) => (
                <SubagentDetail
                  sessionId={sessionId()}
                  subagent={subagent().subagent}
                  items={subagent().items}
                  status={currentStatus()}
                  onclose={() => setSelectedSubagentId(null)}
                />
              )}
            </Show>

            {/* Jump to bottom button */}
            <Show when={showScrollBottom()}>
              <button
                type="button"
                class="bg-card text-foreground border-border hover:bg-accent animate-in fade-in zoom-in-95 absolute bottom-4 left-1/2 z-10 flex -translate-x-1/2 cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1.5 text-[13px] font-medium shadow-md transition-all"
                onClick={scrollToBottom}
              >
                <IconArrowDown class="size-3.5" />
                <span>{m.session_jump_to_latest()}</span>
              </button>
            </Show>
          </div>

          {/* Chat input (Composer) */}
          <div class="border-border shrink-0 border-t px-4 py-2 md:px-8">
            <div class="mx-auto max-w-[760px]">
              <Show when={sessionId()} keyed>
                {(id) => (
                  <Composer
                    sessionId={id}
                    capabilities={capabilities()}
                    node={sessionNode()}
                    status={currentStatus()}
                    permissionPolicy={session()?.permission_policy ?? null}
                    projectId={session()?.project_id ?? ""}
                    agentId={session()?.agent_id ?? ""}
                    resumeRequired={resumeRequired()}
                    onResumeRequiredChange={setResumeRequired}
                    onReady={(api) => {
                      composerApi = api;
                    }}
                  />
                )}
              </Show>
            </div>
          </div>
        </div>

        {/* Desktop workspace dock */}
        <Show when={isDockOpen() && desktopLayout() ? dockContext() : null}>
          {(context) => (
            <>
              <button
                type="button"
                aria-label={m.dock_resize()}
                class={cn(
                  "group border-border hover:bg-signal/50 focus-visible:ring-signal active:bg-signal z-10 -ml-px hidden w-1.5 shrink-0 cursor-col-resize items-center justify-center border-l bg-transparent p-0 outline-none transition-colors select-none focus-visible:ring-1 lg:flex",
                  isResizing() && "bg-signal",
                )}
                onPointerDown={startResize}
                onKeyDown={(e) => {
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
                <span class="bg-muted-foreground/30 group-hover:bg-signal pointer-events-none h-8 w-0.5 rounded-full" />
              </button>

              <div
                class="hidden h-full shrink-0 flex-col overflow-hidden lg:flex"
                style={{ width: `clamp(22rem, ${dock.width}%, 75%)` }}
              >
                <DockHost ctx={context()} items={items()} oncollapse={() => dock.collapse()} />
              </div>
            </>
          )}
        </Show>

        {/* Desktop tool rail */}
        <Show when={!layoutState.rightRailCollapsed && session() !== null}>
          <div class="hidden h-full lg:flex">
            <DockRail
              active={visibleLauncher()}
              onselect={(id) => dock.toggleTool(id)}
              oncollapse={() => layoutState.setRightRailCollapsed(true)}
            />
          </div>
        </Show>

        {/* Mobile dock overlay. The inset panel and the dimmed, blurred backdrop
            make it obvious that this is a layer above the conversation. */}
        <Show when={isDockOpen() && !desktopLayout() ? dockContext() : null}>
          {(context) => (
            <div class="absolute inset-0 z-20 lg:hidden">
              <button
                type="button"
                tabindex="-1"
                aria-label={m.session_close_panel()}
                class="bg-background/60 animate-in fade-in absolute inset-0 cursor-default backdrop-blur-[2px] duration-150"
                onClick={() => dock.collapse()}
              />
              <div class="bg-card border-border animate-in fade-in slide-in-from-bottom-2 absolute inset-x-2 top-6 bottom-2 flex flex-col overflow-hidden rounded-xl border shadow-2xl duration-150">
                <DockHost
                  ctx={context()}
                  items={items()}
                  mobile
                  oncollapse={() => dock.collapse()}
                />
              </div>
            </div>
          )}
        </Show>
      </div>
    </div>
  );
}
