<script lang="ts">
  import { onMount } from "svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { decodeBase64ToBytes, encodeTextToBase64 } from "#lib/base64.ts";
  import type { PtyClientMessage, PtyServerMessage } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { connection } from "#lib/stores/app.svelte.ts";
  import { GhosttyWebAdapter } from "./ghostty-adapter";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";

  let {
    sessionId,
    nodeId,
    path,
    terminalId,
    fill = false,
    active = true,
    onregister,
  }: {
    sessionId: string;
    nodeId: string;
    path: string;
    /** Stable tab identity inside the workspace; identifies this shell on the node. */
    terminalId: string;
    fill?: boolean;
    /** Whether the terminal is the visible dock tab. Hidden terminals stay attached. */
    active?: boolean;
    /** Receives a hook that kills the shell before the tab is removed. */
    onregister?: (kill: () => void) => void;
  } = $props();

  type Status = "disconnected" | "connecting" | "connected" | "exited" | "blocked" | "failed";

  let status = $state<Status>("disconnected");
  let statusMessage = $state<string | null>(null);
  let container = $state<HTMLDivElement | null>(null);

  let adapter: GhosttyWebAdapter | null = null;
  let socket: WebSocket | null = null;
  let disposed = false;
  /** Workspace key of the connected socket (`nodeId` + path). */
  let connectedWorkspace: string | null = null;

  function workspaceKey(): string {
    return `${nodeId}\u0000${path}\u0000${terminalId}`;
  }

  /** Mobile virtual key bar (tapping sends an escape sequence). */
  const KEY_BAR: { label: string; data: string }[] = [
    { label: "Esc", data: "\x1b" },
    { label: "Tab", data: "\t" },
    { label: "Ctrl+C", data: "\x03" },
    { label: "Ctrl+D", data: "\x04" },
    { label: "↑", data: "\x1b[A" },
    { label: "↓", data: "\x1b[B" },
    { label: "→", data: "\x1b[C" },
    { label: "←", data: "\x1b[D" },
    { label: "⏎", data: "\r" },
  ];

  function send(message: PtyClientMessage): boolean {
    if (socket === null || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  function sendInput(data: string): void {
    if (!send({ op: "input", data_b64: encodeTextToBase64(data) })) {
      status = "disconnected";
      statusMessage = m.terminal_input_not_sent();
    }
  }

  function handleMessage(raw: string): void {
    let message: PtyServerMessage;
    try {
      message = JSON.parse(raw) as PtyServerMessage;
    } catch (error) {
      console.error("[fxg] failed to parse pty ws message", error, raw);
      return;
    }
    switch (message.op) {
      case "opened":
        status = "connected";
        statusMessage = null;
        // A hidden terminal has no layout to fit against, so it is sized when
        // its tab becomes active (see the effect below).
        if (active) syncViewport();
        break;
      case "output":
        adapter?.write(decodeBase64ToBytes(message.data_b64));
        break;
      case "exit":
        status = "exited";
        statusMessage = m.terminal_process_exited({
          code:
            message.exit_code === null || message.exit_code === undefined
              ? m.terminal_unknown()
              : String(message.exit_code),
        });
        break;
      case "error":
        if (message.code === "PTY_DISABLED" || message.code === "FORBIDDEN") {
          status = "blocked";
        } else {
          status = "failed";
        }
        statusMessage = message.message;
        break;
    }
  }

  function connect(): void {
    if (socket !== null) return;
    status = "connecting";
    statusMessage = null;

    const ws = new WebSocket(connection.client.wsUrl("/api/v1/pty/ws"));
    socket = ws;

    ws.addEventListener("open", () => {
      if (socket !== ws || disposed) return;
      // Hidden terminals have no measured size; the resize after activation
      // brings the PTY to the real dimensions.
      const dims = active ? (adapter?.fit() ?? { cols: 80, rows: 24 }) : { cols: 80, rows: 24 };
      if (
        !send({
          op: "open",
          node_id: nodeId,
          path,
          session_id: sessionId,
          terminal_id: terminalId,
          cols: dims.cols,
          rows: dims.rows,
          new: false,
        })
      ) {
        status = "failed";
        statusMessage = m.terminal_connect_failed();
      }
    });

    ws.addEventListener("message", (event: MessageEvent) => {
      if (socket !== ws || disposed) return;
      if (typeof event.data === "string") handleMessage(event.data);
    });

    ws.addEventListener("close", () => {
      // Ignore when this socket was already replaced by a new connection
      // (session switch / manual reconnect)
      if (socket !== ws) return;
      socket = null;
      if (disposed) return;
      if (status === "connected" || status === "connecting") {
        status = "disconnected";
        statusMessage = m.terminal_disconnected_notice();
      }
    });

    ws.addEventListener("error", () => {
      if (socket !== ws) return;
      if (status === "connecting") {
        status = "failed";
        statusMessage = m.terminal_connect_failed();
      }
    });
  }

  /** Fits the visible terminal and tells the node about the new size. */
  function syncViewport(): void {
    if (!active) return;
    const dims = adapter?.fit() ?? { cols: 80, rows: 24 };
    send({ op: "resize", cols: dims.cols, rows: dims.rows });
    adapter?.focus();
  }

  function reconnect(): void {
    // The server replays the retained output on attach, so start from a clean
    // screen to avoid duplicating what is already rendered.
    adapter?.clear();
    if (socket !== null) {
      const current = socket;
      socket = null;
      current.close();
    }
    connect();
  }

  // Reconnect only when the *workspace* changes. Switching between sessions of
  // the same directory keeps the running shell (a session is the origin of the
  // terminal, not its owner).
  $effect(() => {
    const key = workspaceKey();
    if (key === connectedWorkspace) {
      return;
    }
    const initial = connectedWorkspace === null;
    connectedWorkspace = key;
    if (initial) return; // onMount performs the initial connection
    reconnect();
  });

  // Refit when the terminal becomes the visible dock tab. When it is hidden,
  // drop focus so keystrokes do not reach an unattended PTY.
  $effect(() => {
    if (active) {
      const frame = requestAnimationFrame(() => syncViewport());
      return () => cancelAnimationFrame(frame);
    }
    const focused = document.activeElement;
    if (container !== null && focused instanceof HTMLElement && container.contains(focused)) {
      focused.blur();
    }
    return;
  });

  onMount(() => {
    if (container !== null) {
      adapter = new GhosttyWebAdapter();
      adapter.mount(container);
    }
    const dataDisposer = adapter?.onData((data) => sendInput(data));
    const resizeDisposer = adapter?.onResize(({ cols, rows }) => {
      send({ op: "resize", cols, rows });
    });
    connect();
    onregister?.(() => {
      send({ op: "kill" });
      socket?.close();
    });

    return () => {
      disposed = true;
      dataDisposer?.dispose();
      resizeDisposer?.dispose();
      socket?.close();
      socket = null;
      adapter?.dispose();
      adapter = null;
    };
  });
</script>

<div class={fill ? "flex h-full min-h-0 flex-col gap-2" : "flex flex-col gap-2"}>
  <div class="flex flex-wrap items-center gap-2 text-[13px]">
    <Badge
      variant={status === "connected"
        ? "secondary"
        : status === "connecting"
          ? "outline"
          : "destructive"}
    >
      {#if status === "connecting"}
        {m.terminal_status_starting()}
      {:else if status === "connected"}
        {m.terminal_status_connected()}
      {:else if status === "disconnected"}
        {m.terminal_status_disconnected()}
      {:else if status === "exited"}
        {m.terminal_status_exited()}
      {:else if status === "blocked"}
        {m.terminal_status_blocked()}
      {:else}
        {m.terminal_status_error()}
      {/if}
    </Badge>
    <Button
      variant="ghost"
      size="sm"
      class="ml-auto"
      onclick={reconnect}
      disabled={status === "connecting"}
    >
      <RefreshCwIcon />
      {m.terminal_reconnect()}
    </Button>
  </div>

  {#if status === "blocked"}
    <div class="text-destructive flex items-start gap-2 rounded-lg border p-3 text-sm">
      <TriangleAlertIcon class="mt-0.5 size-4 shrink-0" />
      <p>
        {m.terminal_blocked_notice_before()}<code>localhost:7860</code
        >{m.terminal_blocked_notice_after()}
        {#if statusMessage !== null}
          <span class="text-muted-foreground block text-[13px]">{statusMessage}</span>
        {/if}
      </p>
    </div>
  {:else if status === "failed" || status === "exited" || status === "disconnected"}
    <div class="flex items-start gap-2 rounded-lg border p-3 text-sm">
      <TriangleAlertIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
      <p>{statusMessage ?? m.terminal_no_connection()}</p>
    </div>
  {/if}

  <div
    bind:this={container}
    class={fill
      ? "min-h-0 w-full flex-1 overflow-hidden rounded-lg border border-border bg-[#17140f] p-1.5"
      : "h-[50svh] min-h-48 w-full overflow-hidden rounded-lg border border-border bg-[#17140f] p-1.5"}
  ></div>

  <!-- Mobile virtual key bar (hidden on desktop) -->
  <div class="flex gap-1 overflow-x-auto pb-1 md:hidden">
    {#each KEY_BAR as key (key.label)}
      <Button
        variant="outline"
        size="sm"
        class="shrink-0 font-mono"
        onclick={() => sendInput(key.data)}
      >
        {key.label}
      </Button>
    {/each}
  </div>
</div>
