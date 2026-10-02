<script lang="ts">
  import { onMount } from 'svelte';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import type { AgentSummary } from '$lib/generated/AgentSummary';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import BotIcon from '@lucide/svelte/icons/bot';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  let agents = $state<AgentSummary[]>([]);
  let loading = $state(false);
  /** 対象ノード (カタログの取得元・操作対象)。 */
  let nodeId = $state('');
  /** 実行中の操作ラベル (例: `opencode2 をインストール中…`)。null = アイドル。 */
  let busyLabel = $state<string | null>(null);
  /** 削除確認の対象。 */
  let removeTarget = $state<AgentSummary | null>(null);

  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const isLocalNode = $derived(sync.connection.role === 'local_node');
  const busy = $derived(busyLabel !== null);

  onMount(() => {
    void loadAgents();
  });

  // オンラインノードの既定選択 (接続直後はノード一覧が空のため反映時に設定)
  $effect(() => {
    if (nodeId === '' && onlineNodes.length > 0) {
      nodeId = onlineNodes[0].node_id;
    }
  });

  function nodeName(id: string): string {
    return sync.nodes.find((node) => node.node_id === id)?.name ?? id;
  }

  function installedOnNode(agent: AgentSummary): boolean {
    if (isLocalNode) return agent.installed_nodes.length > 0;
    return agent.installed_nodes.includes(nodeId);
  }

  /** 個別インストール・削除が必要なバイナリ配布か。 */
  function hasBinary(agent: AgentSummary): boolean {
    return agent.distributions.includes('binary');
  }

  async function loadAgents(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      agents = await sync.connection.client.agents();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loading = false;
    }
  }

  async function installAgent(agent: AgentSummary): Promise<void> {
    if (busy || nodeId === '') return;
    busyLabel = `${agent.id} をインストール中…`;
    try {
      const result = await sync.connection.client.installAgent(nodeId, agent.id);
      toast.success(result.message ?? `${agent.id} をインストールしました`);
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function updateAllAgents(): Promise<void> {
    if (busy || nodeId === '') return;
    busyLabel = '導入済みエージェントを更新中…';
    try {
      const result = await sync.connection.client.updateAgents(nodeId, { agent_id: null });
      toast.success(result.message ?? '更新を実行しました');
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget;
    if (target === null || busy || nodeId === '') return;
    busyLabel = `${target.id} を削除中…`;
    try {
      const result = await sync.connection.client.removeAgent(nodeId, target.id);
      toast.success(result.message ?? `${target.id} を削除しました`);
      removeTarget = null;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <p class="text-muted-foreground text-sm">
      ACP Registry のエージェントをノードへインストール・更新・削除します
    </p>
    <div class="flex flex-wrap items-center gap-1.5">
      <select
        bind:value={nodeId}
        class="border-input bg-background h-8 rounded-md border px-2 text-xs"
        title="操作対象のノード"
      >
        {#each sync.nodes as node (node.node_id)}
          <option value={node.node_id} disabled={!node.is_online}>
            {node.name}{node.is_online ? '' : ' — オフライン'}
          </option>
        {/each}
      </select>
      <Button
        variant="outline"
        size="sm"
        disabled={busy || nodeId === ''}
        onclick={() => void updateAllAgents()}
        title="選択ノードの導入済みエージェントを最新へ更新 (fxg agents update)"
      >
        <RefreshCwIcon />
        更新
      </Button>
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void loadAgents()}>
        <RefreshCwIcon />
        再読込
      </Button>
    </div>
  </div>

  {#if busyLabel !== null}
    <p class="text-muted-foreground text-xs">{busyLabel}</p>
  {/if}

  {#if onlineNodes.length === 0 && sync.nodes.length > 0}
    <p class="text-muted-foreground text-sm">
      オンラインのノードがありません。エージェントの操作にはノードの接続が必要です。
    </p>
  {:else if agents.length === 0}
    <p class="text-muted-foreground text-sm">
      {loading ? 'レジストリを読み込んでいます…' : '取得できたエージェントがありません。'}
    </p>
  {:else}
    {#each agents as agent (agent.id)}
      <Card class="gap-2 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="flex min-w-0 flex-col gap-0.5">
            <div class="flex flex-wrap items-center gap-2">
              <BotIcon class="text-muted-foreground size-4" />
              <h2 class="font-medium">{agent.name}</h2>
              <span class="text-muted-foreground font-mono text-xs">{agent.id}</span>
              <span class="text-muted-foreground text-xs">v{agent.version}</span>
              {#if agent.builtin}
                <Badge variant="secondary">builtin</Badge>
              {/if}
              {#if agent.custom}
                <Badge variant="secondary">custom</Badge>
              {/if}
            </div>
            {#if agent.description}
              <p class="text-muted-foreground text-xs">{agent.description}</p>
            {/if}
            <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
              {#each agent.distributions as distribution (distribution)}
                <Badge variant="outline" class="text-[10px]">{distribution}</Badge>
              {/each}
              {#if isLocalNode && agent.installed_versions.length > 0}
                {#each agent.installed_versions as version (version)}
                  <Badge class="text-[10px]">v{version}</Badge>
                {/each}
              {/if}
            </div>
          </div>

          <div class="flex flex-wrap items-center gap-1.5">
            {#if installedOnNode(agent)}
              <Badge variant="secondary">このノードに導入済み</Badge>
            {/if}
            {#if hasBinary(agent)}
              {#if !installedOnNode(agent)}
                <Button
                  size="sm"
                  disabled={busy || nodeId === ''}
                  onclick={() => void installAgent(agent)}
                >
                  <DownloadIcon />
                  インストール
                </Button>
              {:else}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-destructive"
                  disabled={busy || nodeId === ''}
                  onclick={() => (removeTarget = agent)}
                >
                  <Trash2Icon />
                  <span class="sr-only">{agent.id} を削除</span>
                </Button>
              {/if}
            {:else if !agent.builtin && !agent.custom}
              <span class="text-muted-foreground text-[11px]">npx/uvx (実行時自動取得)</span>
            {/if}
          </div>
        </div>

        {#if agent.installed_nodes.length > 0}
          <p class="text-muted-foreground text-[11px]">
            導入済みノード:
            {agent.installed_nodes.map((id) => nodeName(id)).join(', ')}
          </p>
        {/if}
      </Card>
    {/each}
  {/if}
</div>

<!-- エージェント削除確認 -->
<AlertDialog.Root
  open={removeTarget !== null}
  onOpenChange={(open) => !open && (removeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>エージェントを削除しますか？</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        {nodeName(nodeId)} 上のキャッシュ済みバイナリ <code>{removeTarget?.id}</code>
        を削除します。次回起動時に再ダウンロードされます。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={busy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" disabled={busy} onclick={() => void confirmRemove()}>
        削除
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
