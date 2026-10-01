<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import * as Select from '$lib/components/ui/select';
  import NodeStatusBadge from '$lib/components/NodeStatusBadge.svelte';
  import { getNodeAvailability } from '$lib/node-status';
  import type { NodeSummary } from '$lib/generated/NodeSummary';
  import { sync } from '$lib/stores/app.svelte';
  import type { SessionCapabilities } from '$lib/sync/reducer';
  import type { JsonValue } from '$lib/generated/serde_json/JsonValue';
  import type { SessionControlAction } from '$lib/generated/SessionControlAction';
  import { toast } from 'svelte-sonner';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
  import BanIcon from '@lucide/svelte/icons/ban';
  import SendIcon from '@lucide/svelte/icons/send';
  import SlashIcon from '@lucide/svelte/icons/slash';

  let {
    sessionId,
    capabilities = null,
    node = null
  }: {
    sessionId: string;
    capabilities?: SessionCapabilities | null;
    node?: NodeSummary | null;
  } = $props();

  let text = $state('');
  let sending = $state(false);
  let controlBusy = $state(false);

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
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      controlBusy = false;
    }
  }

  function configValueToString(value: JsonValue): string {
    return typeof value === 'string' ? value : JSON.stringify(value);
  }

  function configOptions(current: JsonValue): { value: string; label: string }[] {
    if (!Array.isArray(current)) return [];
    return current.flatMap((value) => {
      if (value === null || typeof value === 'object') return [];
      return [{ value: configValueToString(value), label: configValueToString(value) }];
    });
  }
</script>

<div class="flex flex-col gap-2 border-t pt-2">
  {#if node && !nodeAvail.isAvailable}
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
    placeholder="プロンプトを入力 (Ctrl+Enter で送信。`/` でスラッシュコマンド)"
    class="border-input bg-background focus-visible:ring-ring/50 w-full resize-y rounded-md border px-3 py-2 text-sm outline-none focus-visible:ring-2"
    disabled={sending}
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
      {@const choices = configOptions(option.options)}
      {#if choices.length > 0}
        <Select.Root
          type="single"
          value={configValueToString(option.current_value)}
          onValueChange={(value) => {
            if (value !== null) {
              void runControl({ action: 'set_config', key: option.key, value });
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
      <Button
        variant="outline"
        size="sm"
        disabled={controlBusy}
        onclick={() => void runControl({ action: 'cancel' })}
      >
        <BanIcon />
        中断
      </Button>
      <Button size="sm" disabled={sending || text.trim() === ''} onclick={() => void send()}>
        <SendIcon />
        {sending ? '送信中…' : '送信'}
      </Button>
    </div>
  </div>
</div>
