<script lang="ts">
  import { onMount } from 'svelte';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import { decodeBase64ToBytes, encodeTextToBase64 } from '$lib/base64';
  import type { PtyClientMessage } from '$lib/generated/PtyClientMessage';
  import type { PtyServerMessage } from '$lib/generated/PtyServerMessage';
  import { connection } from '$lib/stores/app.svelte';
  import { GhosttyWebAdapter } from './ghostty-adapter';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  let { sessionId }: { sessionId: string } = $props();

  type Status = 'disconnected' | 'connecting' | 'connected' | 'exited' | 'blocked' | 'failed';

  let status = $state<Status>('disconnected');
  let statusMessage = $state<string | null>(null);
  let container = $state<HTMLDivElement | null>(null);
  let ptyId = $state<string | null>(null);

  let adapter: GhosttyWebAdapter | null = null;
  let socket: WebSocket | null = null;
  let disposed = false;

  /** モバイル仮想キーバー (タップでエスケープシーケンスを送信)。 */
  const KEY_BAR: { label: string; data: string }[] = [
    { label: 'Esc', data: '\x1b' },
    { label: 'Tab', data: '\t' },
    { label: 'Ctrl+C', data: '\x03' },
    { label: 'Ctrl+D', data: '\x04' },
    { label: '↑', data: '\x1b[A' },
    { label: '↓', data: '\x1b[B' },
    { label: '→', data: '\x1b[C' },
    { label: '←', data: '\x1b[D' },
    { label: '⏎', data: '\r' }
  ];

  function send(message: PtyClientMessage): boolean {
    if (socket === null || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  function sendInput(data: string): void {
    send({ op: 'input', data_b64: encodeTextToBase64(data) });
  }

  function handleMessage(raw: string): void {
    let message: PtyServerMessage;
    try {
      message = JSON.parse(raw) as PtyServerMessage;
    } catch {
      return;
    }
    switch (message.op) {
      case 'spawned':
        ptyId = message.pty_id;
        status = 'connected';
        statusMessage = null;
        adapter?.focus();
        break;
      case 'output':
        adapter?.write(decodeBase64ToBytes(message.data_b64));
        break;
      case 'exit':
        status = 'exited';
        statusMessage = `プロセスが終了しました (exit ${message.exit_code ?? '不明'})`;
        break;
      case 'error':
        if (message.code === 'PTY_DISABLED' || message.code === 'FORBIDDEN') {
          status = 'blocked';
        } else {
          status = 'failed';
        }
        statusMessage = message.message;
        break;
    }
  }

  function connect(): void {
    if (socket !== null) return;
    status = 'connecting';
    statusMessage = null;

    const ws = new WebSocket(connection.client.wsUrl('/api/v1/pty/ws'));
    socket = ws;

    ws.addEventListener('open', () => {
      const dims = adapter?.fit() ?? { cols: 80, rows: 24 };
      send({
        op: 'spawn',
        session_id: sessionId,
        cols: dims.cols,
        rows: dims.rows,
        shell_cmd: null
      });
    });

    ws.addEventListener('message', (event: MessageEvent) => {
      if (typeof event.data === 'string') handleMessage(event.data);
    });

    ws.addEventListener('close', () => {
      if (socket === ws) socket = null;
      if (disposed) return;
      if (status === 'connected' || status === 'connecting') {
        status = 'disconnected';
        statusMessage = 'PTY 接続が切断されました。';
      }
    });

    ws.addEventListener('error', () => {
      if (status === 'connecting') {
        status = 'failed';
        statusMessage = 'PTY WebSocket に接続できませんでした。';
      }
    });
  }

  function reconnect(): void {
    if (socket !== null) {
      const current = socket;
      socket = null;
      current.close();
    }
    ptyId = null;
    connect();
  }

  onMount(() => {
    if (container !== null) {
      adapter = new GhosttyWebAdapter();
      adapter.mount(container);
    }
    const dataDisposer = adapter?.onData((data) => sendInput(data));
    const resizeDisposer = adapter?.onResize(({ cols, rows }) => {
      send({ op: 'resize', cols, rows });
    });
    connect();

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

<div class="flex flex-col gap-2">
  <div class="flex flex-wrap items-center gap-2 text-xs">
    <Badge
      variant={status === 'connected'
        ? 'secondary'
        : status === 'connecting'
          ? 'outline'
          : 'destructive'}
    >
      {#if status === 'connecting'}
        PTY を起動しています…
      {:else if status === 'connected'}
        接続中
      {:else if status === 'disconnected'}
        切断
      {:else if status === 'exited'}
        終了
      {:else if status === 'blocked'}
        無効
      {:else}
        エラー
      {/if}
    </Badge>
    {#if ptyId !== null}
      <span class="text-muted-foreground font-mono">{ptyId}</span>
    {/if}
    <Button
      variant="ghost"
      size="sm"
      class="ml-auto"
      onclick={reconnect}
      disabled={status === 'connecting'}
    >
      <RefreshCwIcon />
      再接続
    </Button>
  </div>

  {#if status === 'blocked'}
    <div class="text-destructive flex items-start gap-2 rounded-lg border p-3 text-sm">
      <TriangleAlertIcon class="mt-0.5 size-4 shrink-0" />
      <p>
        リモート PTY はセキュリティポリシーにより無効化されています。ローカル端末
        (<code>localhost:7860</code> または CLI) からご利用ください。
        {#if statusMessage !== null}
          <span class="text-muted-foreground block text-xs">{statusMessage}</span>
        {/if}
      </p>
    </div>
  {:else if status === 'failed' || status === 'exited' || status === 'disconnected'}
    <div class="flex items-start gap-2 rounded-lg border p-3 text-sm">
      <TriangleAlertIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
      <p>{statusMessage ?? '接続がありません。'}</p>
    </div>
  {/if}

  <div
    bind:this={container}
    class="bg-[#0f172a] h-[50svh] min-h-48 w-full overflow-hidden rounded-lg border"
  ></div>

  <!-- モバイル向け仮想キーバー (デスクトップでは非表示) -->
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
