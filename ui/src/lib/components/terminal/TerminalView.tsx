import { For, Show, createEffect, createMemo, createSignal, onSettled, untrack } from "solid-js";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { decodeBase64ToBytes, encodeTextToBase64 } from "#lib/base64.ts";
import type { PtyClientMessage, PtyServerMessage } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { connection } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconTriangleAlert from "~icons/lucide/triangle-alert";
import { GhosttyWebAdapter } from "./ghostty-adapter.ts";

export interface TerminalViewProps {
  sessionId: string;
  nodeId: string;
  path: string;
  /** Stable tab identity inside the workspace; identifies this shell on the node. */
  terminalId: string;
  fill?: boolean;
  /** Whether the terminal is the visible dock tab. Hidden terminals stay attached. */
  active?: boolean;
  /** Receives a hook that kills the shell before the tab is removed. */
  onRegister?: (kill: () => void) => void;
}

type Status = "disconnected" | "connecting" | "connected" | "exited" | "blocked" | "failed";

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

/** One PTY-backed shell attached to a workspace. */
export default function TerminalView(props: TerminalViewProps) {
  const [status, setStatus] = createSignal<Status>("disconnected");
  const [statusMessage, setStatusMessage] = createSignal<string | null>(null);
  const [container, setContainer] = createSignal<HTMLDivElement>();

  let adapter: GhosttyWebAdapter | null = null;
  let socket: WebSocket | null = null;
  let disposed = false;
  /** Workspace key of the connected socket (`nodeId` + path + terminal id). */
  let connectedWorkspace: string | null = null;

  const isActive = createMemo(() => props.active ?? true);

  const workspaceKey = createMemo(
    () => `${props.nodeId}\u0000${props.path}\u0000${props.terminalId}`,
  );

  function send(message: PtyClientMessage): boolean {
    if (socket === null || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  function sendInput(data: string): void {
    if (!send({ op: "input", data_b64: encodeTextToBase64(data) })) {
      setStatus("disconnected");
      setStatusMessage(m.terminal_input_not_sent());
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
        setStatus("connected");
        setStatusMessage(null);
        // A hidden terminal has no layout to fit against, so it is sized when
        // its tab becomes active (see the effect below).
        if (isActive()) syncViewport();
        break;
      case "output":
        adapter?.write(decodeBase64ToBytes(message.data_b64));
        break;
      case "exit":
        setStatus("exited");
        setStatusMessage(
          m.terminal_process_exited({
            code:
              message.exit_code === null || message.exit_code === undefined
                ? m.terminal_unknown()
                : String(message.exit_code),
          }),
        );
        break;
      case "error":
        if (message.code === "PTY_DISABLED" || message.code === "FORBIDDEN") {
          setStatus("blocked");
        } else {
          setStatus("failed");
        }
        setStatusMessage(message.message);
        break;
    }
  }

  function connect(): void {
    if (socket !== null) return;
    setStatus("connecting");
    setStatusMessage(null);

    const ws = new WebSocket(connection.client.wsUrl("/api/v1/pty/ws"));
    socket = ws;

    ws.addEventListener("open", () => {
      if (socket !== ws || disposed) return;
      // Hidden terminals have no measured size; the resize after activation
      // brings the PTY to the real dimensions.
      const dims = isActive() ? (adapter?.fit() ?? { cols: 80, rows: 24 }) : { cols: 80, rows: 24 };
      if (
        !send({
          op: "open",
          node_id: props.nodeId,
          path: props.path,
          session_id: props.sessionId,
          terminal_id: props.terminalId,
          cols: dims.cols,
          rows: dims.rows,
          new: false,
        })
      ) {
        setStatus("failed");
        setStatusMessage(m.terminal_connect_failed());
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
      if (status() === "connected" || status() === "connecting") {
        setStatus("disconnected");
        setStatusMessage(m.terminal_disconnected_notice());
      }
    });

    ws.addEventListener("error", () => {
      if (socket !== ws) return;
      if (status() === "connecting") {
        setStatus("failed");
        setStatusMessage(m.terminal_connect_failed());
      }
    });
  }

  /** Fits the visible terminal and tells the node about the new size. */
  function syncViewport(): void {
    if (!isActive()) return;
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

  /**
   * Mounts the terminal display once the component's DOM is attached, so the
   * canvas always measures the real container instead of a detached node.
   */
  const mountContainer = (element: HTMLDivElement): void => {
    setContainer(() => element);
  };

  onSettled(() => {
    const element = container();
    if (element !== undefined) {
      const next = new GhosttyWebAdapter();
      next.mount(element);
      next.onData((data) => sendInput(data));
      next.onResize(({ cols, rows }) => {
        send({ op: "resize", cols, rows });
      });
      adapter = next;
      props.onRegister?.(() => {
        send({ op: "kill" });
        socket?.close();
      });
    }
    return () => {
      disposed = true;
      socket?.close();
      socket = null;
      adapter?.dispose();
      adapter = null;
    };
  });

  // Reconnect only when the *workspace* changes. Switching between sessions of
  // the same directory keeps the running shell (a session is the origin of the
  // terminal, not its owner). A hidden terminal is not connected at all, so
  // opening the Files / Changes dock does not spawn a shell.
  createEffect(
    () => ({ key: workspaceKey(), active: isActive() }),
    (source) => {
      if (source.key === connectedWorkspace) return;
      const initial = connectedWorkspace === null;
      connectedWorkspace = source.key;
      if (initial) return; // the connect effect performs the initial connection
      if (!source.active) {
        if (socket !== null) {
          const current = socket;
          socket = null;
          current.close();
        }
        connectedWorkspace = null;
        return;
      }
      untrack(() => reconnect());
    },
  );

  // Connect lazily when the terminal tab becomes visible for the first time.
  createEffect(
    () => isActive(),
    (active) => {
      if (!active) return;
      if (socket === null) {
        connectedWorkspace = workspaceKey();
        untrack(() => connect());
      }
    },
  );

  // Refit when the terminal becomes the visible dock tab. When it is hidden,
  // drop focus so keystrokes do not reach an unattended PTY.
  createEffect(
    () => isActive(),
    (active) => {
      if (active) {
        const frame = requestAnimationFrame(() => syncViewport());
        return () => cancelAnimationFrame(frame);
      }
      const node = container();
      const focused = document.activeElement;
      if (node !== undefined && focused instanceof HTMLElement && node.contains(focused)) {
        focused.blur();
      }
    },
  );

  return (
    <div class={props.fill === true ? "flex h-full min-h-0 flex-col gap-2" : "flex flex-col gap-2"}>
      <div class="flex flex-wrap items-center gap-2 text-[13px]">
        <Badge
          variant={
            status() === "connected"
              ? "secondary"
              : status() === "connecting"
                ? "outline"
                : "destructive"
          }
        >
          {status() === "connecting"
            ? m.terminal_status_starting()
            : status() === "connected"
              ? m.terminal_status_connected()
              : status() === "disconnected"
                ? m.terminal_status_disconnected()
                : status() === "exited"
                  ? m.terminal_status_exited()
                  : status() === "blocked"
                    ? m.terminal_status_blocked()
                    : m.terminal_status_error()}
        </Badge>
        <Button
          variant="ghost"
          size="sm"
          class="ml-auto"
          onClick={reconnect}
          disabled={status() === "connecting"}
        >
          <IconRefreshCw />
          {m.terminal_reconnect()}
        </Button>
      </div>

      <Show when={status() === "blocked"}>
        <div class="text-destructive flex items-start gap-2 rounded-lg border p-3 text-sm">
          <IconTriangleAlert class="mt-0.5 size-4 shrink-0" />
          <p>
            {m.terminal_blocked_notice_before()}
            <code>localhost:7860</code>
            {m.terminal_blocked_notice_after()}
            <Show when={statusMessage() !== null}>
              <span class="text-muted-foreground block text-[13px]">{statusMessage()}</span>
            </Show>
          </p>
        </div>
      </Show>
      <Show when={status() === "failed" || status() === "exited" || status() === "disconnected"}>
        <div class="flex items-start gap-2 rounded-lg border p-3 text-sm">
          <IconTriangleAlert class="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <p>{statusMessage() ?? m.terminal_no_connection()}</p>
        </div>
      </Show>

      <div
        ref={mountContainer}
        class={cn(
          props.fill === true
            ? "min-h-0 w-full flex-1 overflow-hidden rounded-lg border border-border bg-[#17140f] p-1.5"
            : "h-[50svh] min-h-48 w-full overflow-hidden rounded-lg border border-border bg-[#17140f] p-1.5",
        )}
      />

      {/* Mobile virtual key bar (hidden on desktop) */}
      <div class="flex gap-1 overflow-x-auto pb-1 md:hidden">
        <For each={KEY_BAR}>
          {(key) => (
            <Button
              variant="outline"
              size="sm"
              class="shrink-0 font-mono"
              onClick={() => sendInput(key.data)}
            >
              {key.label}
            </Button>
          )}
        </For>
      </div>
    </div>
  );
}
