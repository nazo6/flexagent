<script lang="ts">
  /**
   * Vertical map of the conversation, next to the timeline.
   *
   * One segment per turn, sized by the rendered height of the turn, so the
   * scroll position maps linearly onto the rail. Prompt segments are thick and
   * dark; the agent's answer and folded work are a thin line. Unresolved
   * approvals, questions and error notices are dots.
   *
   * Positions are measured from the DOM (`data-turn-seq` anchors) because
   * markdown, code blocks and folded sections make source length useless as a
   * proxy for height.
   */
  import type { ChatTurn } from "#lib/chat-turns.ts";
  import { turnToolLabel } from "#lib/chat-turns.ts";
  import { m } from "#lib/paraglide/messages.js";
  import ChevronUpIcon from "@lucide/svelte/icons/chevron-up";

  interface Props {
    turns: readonly ChatTurn[];
    /** Scroll container that renders the timeline anchors. */
    container: HTMLElement | null;
    /** Older history exists but is not loaded into the timeline yet. */
    hasOlder?: boolean;
    onloadearlier?: () => void;
    onjump: (turn: ChatTurn) => void;
    /** Emits the turn currently under the reader's viewport. */
    onactive?: (seq: number) => void;
  }

  let { turns, container, hasOlder = false, onloadearlier, onjump, onactive }: Props = $props();

  /** Height of the "older history is not loaded" notch at the top. */
  const CAP_PX = 14;
  /** Very short turns still need a clickable band. */
  const MIN_BAND_PX = 3;

  interface Band {
    seq: number;
    top: number;
    height: number;
    /** Height of the prompt segment at the top of the band. */
    prompt: number;
  }

  interface Marker {
    id: string;
    tone: "danger" | "warn";
    top: number;
  }

  let railEl = $state<HTMLDivElement | null>(null);
  let bands = $state<Band[]>([]);
  let markers = $state<Marker[]>([]);
  let viewTop = $state(0);
  let viewHeight = $state(0);
  /** -1 until the first measurement decides which turn is on screen. */
  let activeIndex = $state(-1);
  /** Change detection for `activeIndex`; kept out of the measured effect's deps. */
  let lastActiveIndex = -1;
  let hover = $state<{ index: number; y: number } | null>(null);
  let dragging = $state(false);

  /** Content-space offsets of the visible turns (read from event handlers only). */
  let anchors: { top: number; end: number }[] = [];
  let contentHeight = 0;

  const activeTurn = $derived(turns[Math.max(0, activeIndex)] ?? null);
  const hoverTurn = $derived(hover === null ? null : (turns[hover.index] ?? null));

  function capHeight(): number {
    return hasOlder ? CAP_PX : 0;
  }

  function layout(): void {
    const scroller = container;
    const rail = railEl;
    if (scroller === null || rail === null) return;
    const railHeight = rail.clientHeight;
    const height = scroller.scrollHeight;
    if (railHeight <= 0 || height <= 0) return;

    contentHeight = height;
    const cap = capHeight();
    const scale = Math.max(1, railHeight - cap) / height;
    const toRail = (y: number): number => cap + y * scale;
    const origin = scroller.getBoundingClientRect().top - scroller.scrollTop;

    const nodes = Array.from(scroller.querySelectorAll<HTMLElement>("[data-turn-seq]"));
    const rects = nodes.map((node) => node.getBoundingClientRect());
    const nextBands: Band[] = [];
    const nextAnchors: { top: number; end: number }[] = [];

    nodes.forEach((node, index) => {
      const rect = rects[index];
      if (rect === undefined) return;
      const top = rect.top - origin;
      const next = rects[index + 1];
      const end = next === undefined ? height : next.top - origin;
      const bandTop = toRail(top);
      const bandEnd = toRail(end);
      nextAnchors.push({ top, end });
      nextBands.push({
        seq: Number(node.dataset.turnSeq),
        top: bandTop,
        height: Math.max(MIN_BAND_PX, bandEnd - bandTop - 1),
        prompt: Math.max(2, toRail(top + rect.height) - bandTop - 1),
      });
    });

    const nextMarkers: Marker[] = [];
    scroller.querySelectorAll<HTMLElement>("[data-attention]").forEach((node, index) => {
      const rect = node.getBoundingClientRect();
      // Collapsed (not mounted) cards have no box; a dot at y=0 would lie.
      if (rect.height === 0 || rect.width === 0) return;
      nextMarkers.push({
        id: `${index}:${node.dataset.attention ?? "warn"}`,
        tone: node.dataset.attention === "danger" ? "danger" : "warn",
        top: toRail(rect.top - origin + Math.min(10, rect.height / 2)),
      });
    });

    anchors = nextAnchors;
    bands = nextBands;
    markers = nextMarkers;
    updateViewport();
  }

  function updateViewport(): void {
    const scroller = container;
    const rail = railEl;
    if (scroller === null || rail === null) return;
    const railHeight = rail.clientHeight;
    const height = scroller.scrollHeight;
    if (railHeight <= 0 || height <= 0) return;
    const cap = capHeight();
    const scale = Math.max(1, railHeight - cap) / height;
    const top = cap + scroller.scrollTop * scale;
    viewTop = top;
    viewHeight = Math.max(8, Math.min(railHeight - top, scroller.clientHeight * scale));
  }

  function updateActive(): void {
    const scroller = container;
    if (scroller === null || anchors.length === 0) return;
    // A turn becomes current once its start passes a reading line near the top.
    // Proportional, so the last turns still activate on tall viewports.
    const probe = scroller.scrollTop + Math.max(64, scroller.clientHeight * 0.25);
    let index = 0;
    anchors.forEach((anchor, position) => {
      if (anchor.top <= probe) index = position;
    });
    // Reaching the end means the tail is on screen, even when the last turns
    // are too short to be scrolled to the top.
    const atBottom = scroller.scrollTop >= scroller.scrollHeight - scroller.clientHeight - 4;
    const clamped = atBottom ? turns.length - 1 : Math.min(index, turns.length - 1);
    if (clamped === lastActiveIndex) return;
    lastActiveIndex = clamped;
    activeIndex = clamped;
    const turn = turns[clamped];
    if (turn !== undefined) onactive?.(turn.seq);
  }

  /** Content coordinate under a pointer position (clamped to the rail). */
  function contentYAt(clientY: number): number {
    const rail = railEl;
    if (rail === null || contentHeight <= 0) return 0;
    const rect = rail.getBoundingClientRect();
    const cap = capHeight();
    const usable = Math.max(1, rect.height - cap);
    const offset = Math.max(0, Math.min(usable, clientY - rect.top - cap));
    return (offset / usable) * contentHeight;
  }

  function scrollToContent(clientY: number, smooth: boolean): void {
    const scroller = container;
    if (scroller === null) return;
    const top = contentYAt(clientY) - scroller.clientHeight / 2;
    scroller.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" });
  }

  function turnIndexAt(clientY: number): number | null {
    const contentY = contentYAt(clientY);
    let found: number | null = null;
    anchors.forEach((anchor, position) => {
      if (contentY >= anchor.top && contentY < anchor.end) found = position;
    });
    return found;
  }

  function updateHover(clientY: number): void {
    const rail = railEl;
    if (rail === null) return;
    const rect = rail.getBoundingClientRect();
    const local = clientY - rect.top;
    if (local < capHeight()) {
      hover = null;
      return;
    }
    const index = turnIndexAt(clientY);
    hover = index === null ? null : { index, y: Math.max(44, Math.min(rect.height - 44, local)) };
  }

  let pointerId: number | null = null;
  /** Pointer-down position, so a plain click jumps instead of scrubbing. */
  let pressY: number | null = null;
  let pressTurn: number | null = null;
  let moved = false;

  function handlePointerDown(event: PointerEvent): void {
    if (event.button !== 0) return;
    const rail = railEl;
    if (rail === null) return;
    const rect = rail.getBoundingClientRect();
    if (event.clientY - rect.top < capHeight()) return;
    pointerId = event.pointerId;
    pressY = event.clientY;
    pressTurn = turnIndexAt(event.clientY);
    moved = false;
    hover = null;
    rail.setPointerCapture(event.pointerId);
    rail.focus({ preventScroll: true });
    event.preventDefault();
  }

  function handlePointerMove(event: PointerEvent): void {
    if (pressY !== null && !dragging && Math.abs(event.clientY - pressY) > 4) {
      moved = true;
      dragging = true;
    }
    if (dragging) {
      // Scrubbing is continuous and centers the point under the pointer.
      scrollToContent(event.clientY, false);
      return;
    }
    if (pressY === null) updateHover(event.clientY);
  }

  function handlePointerUp(event: PointerEvent): void {
    if (event.pointerId !== pointerId) return;
    if (!moved && pressTurn !== null) {
      // A click picks a turn: align its start with the viewport top (like the
      // outline), so the scroll spy agrees with what was clicked.
      const turn = turns[pressTurn];
      if (turn !== undefined) onjump(turn);
    }
    pointerId = null;
    pressY = null;
    pressTurn = null;
    moved = false;
    dragging = false;
  }

  function handleKeydown(event: KeyboardEvent): void {
    const last = turns.length - 1;
    if (last < 0) return;
    let next: number;
    switch (event.key) {
      case "ArrowDown":
      case "ArrowRight":
      case "PageDown":
        next = Math.min(last, activeIndex + 1);
        break;
      case "ArrowUp":
      case "ArrowLeft":
      case "PageUp":
        next = Math.max(0, activeIndex - 1);
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = last;
        break;
      default:
        return;
    }
    event.preventDefault();
    const turn = turns[next];
    if (turn !== undefined) onjump(turn);
  }

  $effect(() => {
    const scroller = container;
    // Re-measure when the container mounts, when turns appear or leave
    // (paging / a new prompt) and when the unloaded notch changes the origin.
    const turnCount = turns.length;
    if (scroller === null || turnCount === 0) return;

    let frame = 0;
    const run = (): void => {
      frame = 0;
      layout();
      updateActive();
    };
    const runScroll = (): void => {
      frame = 0;
      updateViewport();
      updateActive();
    };
    const schedule = (): void => {
      if (frame === 0) frame = requestAnimationFrame(run);
    };
    const scheduleScroll = (): void => {
      if (frame === 0) frame = requestAnimationFrame(runScroll);
    };

    run();

    // Content growth (streaming, expanding a folded block) changes heights;
    // the viewport resize changes the rail scale. Scrolling only moves the
    // bracket, so it does not re-measure the timeline.
    const observer = new ResizeObserver(schedule);
    observer.observe(scroller);
    const inner = scroller.firstElementChild;
    if (inner !== null) observer.observe(inner);
    scroller.addEventListener("scroll", scheduleScroll, { passive: true });
    window.addEventListener("resize", schedule);

    return () => {
      observer.disconnect();
      scroller.removeEventListener("scroll", scheduleScroll);
      window.removeEventListener("resize", schedule);
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  });
</script>

<div
  class="absolute top-3 right-1.5 bottom-3 z-10 w-5 md:right-[max(1.25rem,calc(50%_-_25.75rem))]"
>
  <div
    bind:this={railEl}
    class="group relative h-full w-full touch-none cursor-pointer rounded-md outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/40"
    role="slider"
    tabindex="0"
    aria-label={m.chat_turns_nav()}
    aria-valuemin={1}
    aria-valuemax={Math.max(1, turns.length)}
    aria-valuenow={Math.max(1, activeIndex + 1)}
    aria-valuetext={activeTurn === null
      ? undefined
      : activeTurn.title === ""
        ? m.chat_turns_untitled()
        : activeTurn.title}
    onpointerdown={handlePointerDown}
    onpointermove={handlePointerMove}
    onpointerup={handlePointerUp}
    onpointercancel={handlePointerUp}
    onpointerleave={() => {
      if (!dragging) hover = null;
    }}
    onkeydown={handleKeydown}
  >
    {#if hasOlder}
      <button
        type="button"
        class="text-muted-foreground/70 hover:text-signal absolute top-0 left-1/2 flex size-3.5 -translate-x-1/2 items-center justify-center rounded-full"
        title={m.chat_turns_load_earlier()}
        aria-label={m.chat_turns_load_earlier()}
        onpointerdown={(event) => event.stopPropagation()}
        onclick={() => onloadearlier?.()}
      >
        <ChevronUpIcon class="size-3" />
      </button>
    {/if}

    <div class="pointer-events-none absolute inset-0">
      {#each bands as band (band.seq)}
        <div
          class="bg-ink/15 absolute left-1/2 w-[3px] -translate-x-1/2 rounded-full"
          style:top="{band.top}px"
          style:height="{band.height}px"
        ></div>
        <div
          class="bg-ink/55 absolute left-1/2 w-[6px] -translate-x-1/2 rounded-full"
          style:top="{band.top}px"
          style:height="{band.prompt}px"
        ></div>
      {/each}

      {#each markers as marker (marker.id)}
        <div
          class="ring-background absolute left-1/2 size-[7px] -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 {marker.tone ===
          'danger'
            ? 'bg-danger'
            : 'bg-warn'}"
          style:top="{marker.top}px"
        ></div>
      {/each}

      <div
        class="border-signal bg-signal/10 absolute -inset-x-1 rounded-[4px] border-[1.5px]"
        style:top="{viewTop}px"
        style:height="{viewHeight}px"
      ></div>
    </div>

    {#if hover !== null && hoverTurn !== null}
      <div
        class="bg-popover border-border pointer-events-none absolute right-full z-30 mr-2 w-56 -translate-y-1/2 rounded-lg border px-2.5 py-2 shadow-md"
        style:top="{hover.y}px"
      >
        <p class="text-muted-foreground desk-mono text-[10px]">
          {hover.index + 1} / {turns.length}
        </p>
        <p class="mt-0.5 line-clamp-2 text-[12px] leading-snug">
          {hoverTurn.title === "" ? m.chat_turns_untitled() : hoverTurn.title}
        </p>
        {#if hoverTurn.tools > 0}
          <p class="text-muted-foreground mt-1 text-[10.5px]">
            {turnToolLabel(hoverTurn.tools)}
          </p>
        {/if}
      </div>
    {/if}
  </div>
</div>
