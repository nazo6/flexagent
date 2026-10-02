<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import ComposerSelect from '$lib/components/chat/ComposerSelect.svelte';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import { configChoices, resolveConfigChoice, selectedConfigValue } from '$lib/config-options';
  import { getNodeAvailability } from '$lib/node-status';
  import type { NodeSummary } from '$lib/generated/NodeSummary';
  import { sync } from '$lib/stores/app.svelte';
  import type { SessionCapabilities } from '$lib/sync/reducer';
  import type { SessionControlAction } from '$lib/generated/SessionControlAction';
  import type { SessionStatus } from '$lib/generated/SessionStatus';
  import { toast } from 'svelte-sonner';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
  import BanIcon from '@lucide/svelte/icons/ban';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import SendIcon from '@lucide/svelte/icons/send';
  import SlashIcon from '@lucide/svelte/icons/slash';

  let {
    sessionId,
    capabilities = null,
    node = null,
    status = null,
    /**
     * 送信時にネイティブ復元非対応 (`RESUME_REQUIRED`) だった。
     * 通常は false (停止中でも送信で自動再開される) で、真のときだけ
     * 履歴 Replay での再開 (Resume) を案内する。
     */
    resumeRequired = $bindable(false)
  }: {
    sessionId: string;
    capabilities?: SessionCapabilities | null;
    node?: NodeSummary | null;
    status?: SessionStatus | null;
    resumeRequired?: boolean;
  } = $props();

  let text = $state('');
  let sending = $state(false);
  let controlBusy = $state(false);
  let resuming = $state(false);

  const isRunning = $derived(
    status === 'running' || status === 'waiting_permission' || status === 'waiting_input'
  );
  const isStopped = $derived(
    status === 'stopped' || status === 'error'
  );

  // セッションが稼働状態になったら再開提案を閉じる
  $effect(() => {
    if (
      status === 'idle' ||
      status === 'running' ||
      status === 'waiting_permission' ||
      status === 'waiting_input'
    ) {
      resumeRequired = false;
    }
  });

  const nodeAvail = $derived(getNodeAvailability(node));

  /** モードセレクタの選択肢 (value = mode_id、表示はエージェントが返す名前)。 */
  const modeItems = $derived(
    (capabilities?.availableModes ?? []).map((mode) => ({
      value: mode.mode_id,
      label: mode.name.trim() === '' ? mode.mode_id : mode.name,
      description: mode.description
    }))
  );

  /** 現在のモード (選択肢に存在しない ID は未選択として扱う)。 */
  const currentMode = $derived(capabilities?.currentMode ?? null);
  const selectedMode = $derived(
    currentMode !== null && modeItems.some((item) => item.value === currentMode)
      ? currentMode
      : undefined
  );

  const slashQuery = $derived(
    text.startsWith('/') && !text.includes(' ') ? text.slice(1).toLowerCase() : null
  );

  const commandMatches = $derived(
    capabilities && slashQuery !== null
      ? capabilities.availableCommands
          .filter((command) => command.name.toLowerCase().startsWith(slashQuery))
          .slice(0, 8)
      : []
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
        if (result.code === 'RESUME_REQUIRED') {
          // 停止済みセッションの自動再開 (ネイティブ復元) に非対応。
          // 入力は保持したまま、履歴 Replay での再開を提案する
          resumeRequired = true;
        } else {
          toast.error(result.error ?? result.code ?? '送信に失敗しました');
        }
        return;
      }
      text = '';
      resumeRequired = false;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      sending = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
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
        toast.error(result.error ?? result.code ?? '操作に失敗しました');
      } else if (action.action === 'cancel') {
        toast.info('中断リクエストを送信しました');
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      controlBusy = false;
    }
  }

  /** 停止済みセッションを再開する (履歴 Replay フォールバック含む)。 */
  async function resumeSession(): Promise<boolean> {
    if (sending || resuming) return false;
    resuming = true;
    try {
      const result = await sync.resumeSession(sessionId);
      if (result.context_restored) {
        toast.success('セッションを再開しました (エージェントのコンテキストを復元)');
      } else {
        toast.success('セッションを再開しました (履歴を引き継いで継続)');
      }
      return true;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      resuming = false;
    }
  }

  /** ネイティブ復元非対応の停止セッションを履歴 Replay で再開し、入力を送信する。 */
  async function resumeAndSend() {
    if (sending || text.trim() === '') return;
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
        このセッションは停止しています。送信すると自動で再開します（ネイティブ復元非対応時は履歴を引き継いで再開）。
      </span>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-6 text-xs"
        disabled={sending || resuming}
        title="再開するとスラッシュコマンドなどの最新の能力情報も再取得します"
        onclick={() => void resumeSession()}
      >
        {#if resuming}
          <LoaderCircleIcon class="size-3.5 animate-spin" />
        {/if}
        {resuming ? '再開中…' : '再開'}
      </Button>
    </div>
  {:else if node && !nodeAvail.isAvailable}
    <div class="bg-destructive/10 text-destructive flex items-center gap-2 rounded-md px-2.5 py-1 text-xs">
      <AlertTriangleIcon class="size-3.5 shrink-0" />
      <span>実行ノード ({node.name}) は現在{nodeAvail.statusText}です。送信したプロンプトはノード復帰時に処理されます。</span>
    </div>
  {/if}

  {#if resumeRequired}
    <div
      class="bg-muted text-muted-foreground flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1 text-xs"
    >
      <AlertTriangleIcon class="size-3.5 shrink-0 text-amber-500/80" />
      <span>このエージェントはネイティブ復元に対応していません。履歴を引き継いで再開してから送信できます。</span>
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
        {resuming ? '再開中…' : '履歴を引き継いで再開して送信'}
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
      ? '停止中: 送信すると自動で再開します (Ctrl+Enter で送信)'
      : isRunning
        ? '次のプロンプトを入力 (送信するとキューに追加されます。Ctrl+Enter で送信)'
        : 'プロンプトを入力 (Ctrl+Enter で送信。`/` でスラッシュコマンド)'}
    class="border-input bg-background focus-visible:ring-ring/50 w-full resize-y rounded-md border px-3 py-2 text-sm outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
    disabled={sending}
  ></textarea>

  <div class="flex flex-wrap items-center gap-2">
    {#if node}
      <NodeStatusBadge {node} showNodeName={true} class="mr-1 hidden sm:inline-flex" />
    {/if}
    {#if capabilities && capabilities.availableModes.length > 0}
      <ComposerSelect
        label="モード"
        placeholder="既定"
        value={selectedMode}
        items={modeItems}
        onchange={(value) => void runControl({ action: 'set_mode', mode_id: value })}
      />
    {/if}

    {#each capabilities?.configOptions ?? [] as option (option.key)}
      {@const choices = configChoices(option.options)}
      {#if choices.length > 0}
        <ComposerSelect
          label={option.name}
          placeholder="既定"
          value={selectedConfigValue(choices, option.current_value)}
          items={choices.map((choice) => ({ value: choice.value, label: choice.label }))}
          onchange={(value) =>
            void runControl({
              action: 'set_config',
              key: option.key,
              value: resolveConfigChoice(choices, value)
            })}
        />
      {/if}
    {/each}

    <div class="ml-auto flex items-center gap-2">
      {#if !sync.wsConnected}
        <span class="text-destructive text-xs">接続が切断されています</span>
      {/if}
      {#if isRunning}
        <Button
          variant="destructive"
          size="sm"
          disabled={controlBusy}
          title="エージェントの実行を中断します"
          onclick={() => void runControl({ action: 'cancel' })}
        >
          <BanIcon class="size-3.5" />
          {controlBusy ? '中断中…' : '中断'}
        </Button>
      {/if}
      <Button
        size="sm"
        disabled={sending || text.trim() === ''}
        onclick={() => void send()}
      >
        {#if sending}
          <LoaderCircleIcon class="size-3.5 animate-spin" />
        {:else}
          <SendIcon class="size-3.5" />
        {/if}
        {sending ? '送信中…' : isRunning ? 'キューに追加' : '送信'}
      </Button>
    </div>
  </div>
</div>
