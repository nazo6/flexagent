<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import * as Select from '$lib/components/ui/select';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import { configChoices, configValueToString, resolveConfigChoice } from '$lib/config-options';
  import { getNodeAvailability } from '$lib/node-status';
  import type { NodeSummary } from '$lib/generated/NodeSummary';
  import { sync } from '$lib/stores/app.svelte';
  import type { SessionCapabilities } from '$lib/sync/reducer';
  import type { SessionControlAction } from '$lib/generated/SessionControlAction';
  import type { SessionStatus } from '$lib/generated/SessionStatus';
  import { toast } from 'svelte-sonner';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
  import BanIcon from '@lucide/svelte/icons/ban';
  import SendIcon from '@lucide/svelte/icons/send';
  import SlashIcon from '@lucide/svelte/icons/slash';

  let {
    sessionId,
    capabilities = null,
    node = null,
    status = null
  }: {
    sessionId: string;
    capabilities?: SessionCapabilities | null;
    node?: NodeSummary | null;
    status?: SessionStatus | null;
  } = $props();

  let text = $state('');
  let sending = $state(false);
  let controlBusy = $state(false);
  let resuming = $state(false);

  const isRunning = $derived(
    status === 'running' || status === 'waiting_permission'
  );
  const isStopped = $derived(
    status === 'stopped' || status === 'error'
  );

  const nodeAvail = $derived(getNodeAvailability(node));

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
        toast.error(result.error ?? result.code ?? '送信に失敗しました');
        return;
      }
      text = '';
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      sending = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      if (!isStopped) {
        void send();
      }
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

  /** 停止済みセッションを再開し、続きから操作できるようにする。 */
  async function resume() {
    if (resuming) return;
    resuming = true;
    try {
      const result = await sync.resumeSession(sessionId);
      if (result.context_restored) {
        toast.success('セッションを再開しました (エージェントのコンテキストを復元)');
      } else {
        toast.success('セッションを再開しました (履歴を引き継いで継続)');
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      resuming = false;
    }
  }
</script>

<div class="flex flex-col gap-2 border-t pt-2">
  {#if isStopped}
    <div
      class="bg-muted text-muted-foreground flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1 text-xs"
    >
      <AlertTriangleIcon class="size-3.5 shrink-0" />
      <span>このセッションは停止しています。再開すると会話の続きから操作できます。</span>
      <Button
        variant="outline"
        size="sm"
        class="ml-auto h-6 text-xs"
        disabled={resuming}
        onclick={resume}
      >
        再開 (Resume)
      </Button>
    </div>
  {:else if node && !nodeAvail.isAvailable}
    <div class="bg-destructive/10 text-destructive flex items-center gap-2 rounded-md px-2.5 py-1 text-xs">
      <AlertTriangleIcon class="size-3.5 shrink-0" />
      <span>実行ノード ({node.name}) は現在{nodeAvail.statusText}です。送信したプロンプトはノード復帰時に処理されます。</span>
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
      ? 'セッションは停止しています'
      : isRunning
        ? '次のプロンプトを入力 (送信するとキューに追加されます。Ctrl+Enter で送信)'
        : 'プロンプトを入力 (Ctrl+Enter で送信。`/` でスラッシュコマンド)'}
    class="border-input bg-background focus-visible:ring-ring/50 w-full resize-y rounded-md border px-3 py-2 text-sm outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
    disabled={sending || isStopped}
  ></textarea>

  <div class="flex flex-wrap items-center gap-2">
    {#if node}
      <NodeStatusBadge {node} showNodeName={true} class="mr-1 hidden sm:inline-flex" />
    {/if}
    {#if capabilities && capabilities.availableModes.length > 0}
      <Select.Root
        type="single"
        value={capabilities.currentMode ?? undefined}
        onValueChange={(value) => {
          if (value) void runControl({ action: 'set_mode', mode_id: value });
        }}
      >
        <Select.Trigger size="sm" class="w-36">
          <Select.Value placeholder="モード" />
        </Select.Trigger>
        <Select.Content>
          {#each capabilities.availableModes as mode (mode.mode_id)}
            <Select.Item value={mode.mode_id} label={mode.name ?? mode.mode_id}>
              {mode.name}
            </Select.Item>
          {/each}
        </Select.Content>
      </Select.Root>
    {/if}

    {#each capabilities?.configOptions ?? [] as option (option.key)}
      {@const choices = configChoices(option.options)}
      {#if choices.length > 0}
        <Select.Root
          type="single"
          value={configValueToString(option.current_value)}
          onValueChange={(value) => {
            if (value !== null) {
              void runControl({
                action: 'set_config',
                key: option.key,
                value: resolveConfigChoice(choices, value)
              });
            }
          }}
        >
          <Select.Trigger size="sm" class="max-w-48">
            <Select.Value placeholder={option.name} />
          </Select.Trigger>
          <Select.Content>
            {#each choices as choice (choice.value)}
              <Select.Item value={choice.value} label={`${option.name}: ${choice.label}`}>
                {choice.label}
              </Select.Item>
            {/each}
          </Select.Content>
        </Select.Root>
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
        disabled={sending || isStopped || text.trim() === ''}
        onclick={() => void send()}
      >
        <SendIcon class="size-3.5" />
        {sending ? '送信中…' : isRunning ? 'キューに追加' : '送信'}
      </Button>
    </div>
  </div>
</div>
