<script lang="ts">
  import { onMount } from "svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import { Card } from "$lib/components/ui/card";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { agentSourceLabel, agentsAvailableOn, isPackageEnabled } from "$lib/agents";
  import type { AgentSummary } from "$lib/generated/AgentSummary";
  import { sync } from "$lib/stores/app.svelte";
  import { toast } from "svelte-sonner";
  import BotIcon from "@lucide/svelte/icons/bot";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import PackageIcon from "@lucide/svelte/icons/package";
  import PowerOffIcon from "@lucide/svelte/icons/power-off";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let agents = $state<AgentSummary[]>([]);
  let loading = $state(false);
  /** 対象ノード (カタログの取得元・操作対象)。 */
  let nodeId = $state("");
  /** 実行中の操作ラベル (例: `opencode2 をインストール中…`)。null = アイドル。 */
  let busyLabel = $state<string | null>(null);
  /** 削除確認の対象。 */
  let removeTarget = $state<AgentSummary | null>(null);

  // カタログ (このノードで未導入のレジストリエージェント) ダイアログ
  let catalogOpen = $state(false);
  let catalogQuery = $state("");

  // カスタムエージェント定義フォーム (null = 新規作成)
  let customOpen = $state(false);
  let customBusy = $state(false);
  let customEditId = $state<string | null>(null);
  let customForm = $state({ id: "", name: "", command: "", args: "", env: "" });

  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const busy = $derived(busyLabel !== null);

  /** このノードで使えるエージェント (npx/uvx は有効化したものだけ含まれる)。 */
  const available = $derived(agentsAvailableOn(agents, nodeId));
  /** このノードでまだ使えないレジストリエージェント。 */
  const catalogOnly = $derived(
    agents
      .filter((agent) => agent.registry && !isAvailableOn(agent))
      .filter((agent) => matchesQuery(agent, catalogQuery)),
  );

  onMount(() => {
    void loadAgents();
  });

  // オンラインノードの既定選択 (接続直後はノード一覧が空のため反映時に設定)
  $effect(() => {
    if (nodeId === "" && onlineNodes.length > 0) {
      nodeId = onlineNodes[0].node_id;
    }
  });

  function isAvailableOn(agent: AgentSummary): boolean {
    return agent.installed_nodes.includes(nodeId);
  }

  function matchesQuery(agent: AgentSummary, query: string): boolean {
    const needle = query.trim().toLowerCase();
    if (needle === "") return true;
    return [agent.id, agent.name, agent.description ?? ""].some((value) =>
      value.toLowerCase().includes(needle),
    );
  }

  function nodeName(id: string): string {
    return sync.nodes.find((node) => node.node_id === id)?.name ?? id;
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

  async function installAgent(agent: AgentSummary, closeCatalog = false): Promise<void> {
    if (busy || nodeId === "") return;
    busyLabel = `${agent.id} をインストール中…`;
    try {
      const result = await sync.connection.client.installAgent(nodeId, agent.id);
      toast.success(result.message ?? `${agent.id} をインストールしました`);
      if (closeCatalog) catalogOpen = false;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function setPackageEnabled(agent: AgentSummary, enabled: boolean): Promise<void> {
    if (busy || nodeId === "") return;
    busyLabel = `${agent.id} の npx/uvx 利用を${enabled ? "有効化" : "無効化"}中…`;
    try {
      const result = enabled
        ? await sync.connection.client.enablePackageAgent(nodeId, agent.id)
        : await sync.connection.client.disablePackageAgent(nodeId, agent.id);
      toast.success(
        result.message ?? `${agent.id} の npx/uvx 利用を${enabled ? "有効化" : "無効化"}しました`,
      );
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function updateAllAgents(): Promise<void> {
    if (busy || nodeId === "") return;
    busyLabel = "導入済みエージェントを更新中…";
    try {
      const result = await sync.connection.client.updateAgents(nodeId, {
        agent_id: null,
      });
      toast.success(result.message ?? "更新を実行しました");
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget;
    if (target === null || busy || nodeId === "") return;
    const isCustom = target.custom_config !== null;
    busyLabel = `${target.id} を削除中…`;
    try {
      const result = isCustom
        ? await sync.connection.client.removeCustomAgent(nodeId, target.id)
        : await sync.connection.client.removeAgent(nodeId, target.id);
      toast.success(result.message ?? `${target.id} を削除しました`);
      removeTarget = null;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyLabel = null;
    }
  }

  /** カスタム定義フォームを開く (agent 省略時は新規作成)。 */
  function openCustom(agent: AgentSummary | null): void {
    const config = agent?.custom_config ?? null;
    customEditId = agent?.id ?? null;
    customForm = {
      id: agent?.id ?? "",
      name: config?.name ?? "",
      command: config?.command ?? "",
      args: config?.args.join(" ") ?? "",
      env: formatEnv(config?.env ?? {}),
    };
    customOpen = true;
  }

  function formatEnv(env: Record<string, string>): string {
    return Object.entries(env)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n");
  }

  function parseEnv(text: string): Record<string, string> {
    const env: Record<string, string> = {};
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (trimmed === "" || trimmed.startsWith("#")) continue;
      const index = trimmed.indexOf("=");
      if (index <= 0) continue;
      env[trimmed.slice(0, index).trim()] = trimmed.slice(index + 1).trim();
    }
    return env;
  }

  async function submitCustom(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (customBusy || nodeId === "") return;
    const id = (customEditId ?? customForm.id).trim();
    if (id === "") {
      toast.error("エージェントIDを入力してください");
      return;
    }
    if (customForm.command.trim() === "") {
      toast.error("実行コマンドを入力してください");
      return;
    }
    customBusy = true;
    try {
      const result = await sync.connection.client.upsertCustomAgent(nodeId, id, {
        name: customForm.name.trim() === "" ? null : customForm.name.trim(),
        command: customForm.command.trim(),
        args: customForm.args.split(/\s+/).filter((value) => value !== ""),
        env: parseEnv(customForm.env),
      });
      toast.success(result.message ?? `カスタムエージェント ${id} を保存しました`);
      customOpen = false;
      await Promise.all([loadAgents(), sync.refreshNodes()]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      customBusy = false;
    }
  }
</script>

<div class="flex flex-col gap-3">
  <div class="flex flex-wrap items-center justify-between gap-2">
    <p class="text-muted-foreground text-sm">
      このノードで使える ACP エージェントの一覧です。fxg 管理のインストールに加え、PATH
      上の既存インストールとカスタム定義も検出します。
    </p>
    <div class="flex flex-wrap items-center gap-1.5">
      <select
        bind:value={nodeId}
        class="border-input bg-background h-8 rounded-md border px-2 text-xs"
        title="操作対象のノード"
      >
        {#each sync.nodes as node (node.node_id)}
          <option value={node.node_id} disabled={!node.is_online}>
            {node.name}{node.is_online ? "" : " — オフライン"}
          </option>
        {/each}
      </select>
      <Button
        variant="outline"
        size="sm"
        disabled={busy || nodeId === ""}
        onclick={() => void updateAllAgents()}
        title="選択ノードの fxg 管理エージェントを最新へ更新 (fxg agents update)"
      >
        <RefreshCwIcon />
        更新
      </Button>
      <Button variant="outline" size="sm" disabled={loading} onclick={() => void loadAgents()}>
        <RefreshCwIcon />
        再読込
      </Button>
      <Button size="sm" disabled={nodeId === ""} onclick={() => openCustom(null)}>
        <PlusIcon />
        カスタム追加
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
  {:else if nodeId === ""}
    <p class="text-muted-foreground text-sm">ノードを選択してください。</p>
  {:else if loading && agents.length === 0}
    <p class="text-muted-foreground text-sm">レジストリを読み込んでいます…</p>
  {:else if available.length === 0}
    <p class="text-muted-foreground text-sm">
      このノードで使えるエージェントがありません。「カタログから追加」または「カスタム追加」から登録してください。
    </p>
  {:else}
    {#each available as agent (agent.id)}
      <Card class="gap-2 p-4">
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="flex min-w-0 flex-col gap-0.5">
            <div class="flex flex-wrap items-center gap-2">
              <BotIcon class="text-muted-foreground size-4" />
              <h2 class="font-medium">{agent.name}</h2>
              <span class="text-muted-foreground font-mono text-xs">{agent.id}</span>
              {#if agent.version !== "-"}
                <span class="text-muted-foreground text-xs">v{agent.version}</span>
              {/if}
            </div>
            {#if agent.description}
              <p class="text-muted-foreground text-xs">{agent.description}</p>
            {/if}
            <div class="mt-0.5 flex flex-wrap items-center gap-1.5">
              {#each agent.sources as source (source)}
                <Badge variant="outline" class="text-[10px]">{agentSourceLabel(source)}</Badge>
              {/each}
              {#each agent.managed_versions as version (version)}
                <Badge class="text-[10px]">v{version}</Badge>
              {/each}
            </div>
            {#if agent.external_path}
              <p class="text-muted-foreground font-mono text-[11px] break-all">
                {agent.external_path}
              </p>
            {/if}
          </div>

          <div class="flex flex-wrap items-center gap-1.5">
            {#if agent.installable && agent.managed_versions.length === 0}
              <Button
                variant="outline"
                size="sm"
                disabled={busy || nodeId === ""}
                onclick={() => void installAgent(agent)}
                title="PATH 上の既存インストールを優先して使います。fxg 管理に切り替えるにはインストールしてください"
              >
                <DownloadIcon />
                fxg でインストール
              </Button>
            {/if}
            {#if isPackageEnabled(agent)}
              <Button
                variant="ghost"
                size="sm"
                class="text-muted-foreground"
                disabled={busy || nodeId === ""}
                onclick={() => void setPackageEnabled(agent, false)}
                title="npx/uvx 経由の利用を一覧から外します (実行ファイルは削除しません)"
              >
                <PowerOffIcon />
                npx/uvx を無効化
              </Button>
            {/if}
            {#if agent.custom_config !== null}
              <Button
                variant="ghost"
                size="icon-sm"
                disabled={busy || nodeId === ""}
                onclick={() => openCustom(agent)}
                title="カスタム定義を編集"
              >
                <PencilIcon />
                <span class="sr-only">{agent.id} を編集</span>
              </Button>
            {/if}
            {#if agent.custom_config !== null || agent.managed_versions.length > 0}
              <Button
                variant="ghost"
                size="icon-sm"
                class="text-muted-foreground hover:text-destructive"
                disabled={busy || nodeId === ""}
                onclick={() => (removeTarget = agent)}
              >
                <Trash2Icon />
                <span class="sr-only">{agent.id} を削除</span>
              </Button>
            {/if}
          </div>
        </div>

        {#if agent.installed_nodes.length > 1}
          <p class="text-muted-foreground text-[11px]">
            利用可能なノード:
            {agent.installed_nodes.map((id) => nodeName(id)).join(", ")}
          </p>
        {/if}
      </Card>
    {/each}
  {/if}

  <div class="mt-1">
    <Button
      variant="outline"
      size="sm"
      disabled={nodeId === ""}
      onclick={() => (catalogOpen = true)}
    >
      <PlusIcon />
      カタログから追加 ({catalogOnly.length})
    </Button>
  </div>
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
        {#if removeTarget?.custom_config !== null}
          <code>{removeTarget?.id}</code>
          のカスタム定義を削除します。設定ファイル (<code>config.toml</code>)
          側に同名の定義がある場合はそちらが残ります。
        {:else}
          {nodeName(nodeId)} 上の fxg 管理バイナリ
          <code>{removeTarget?.id}</code>
          を削除します。PATH 上の既存インストールには影響しません。
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={busy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={busy}
        onclick={() => void confirmRemove()}
      >
        削除
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- カタログ (未導入) ダイアログ -->
<Dialog.Root open={catalogOpen} onOpenChange={(open) => (catalogOpen = open)}>
  <Dialog.Content class="sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>カタログから追加</Dialog.Title>
      <Dialog.Description>
        ACP Registry に登録されている未導入のエージェントです。インストール (または npx/uvx
        を有効化) すると {nodeName(nodeId)} で利用できるようになります。
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3">
      <div class="relative">
        <SearchIcon class="text-muted-foreground absolute top-2 left-2 size-4" />
        <Input
          bind:value={catalogQuery}
          placeholder="名前・ID・説明で検索"
          class="h-8 pl-8 text-xs"
        />
      </div>
      <div class="flex max-h-[60vh] flex-col gap-2 overflow-y-auto">
        {#if catalogOnly.length === 0}
          <p class="text-muted-foreground text-sm">
            {catalogQuery.trim() === ""
              ? "未導入のエージェントはありません。"
              : "一致するエージェントがありません。"}
          </p>
        {:else}
          {#each catalogOnly as agent (agent.id)}
            <div class="flex flex-wrap items-start justify-between gap-2 rounded-md border p-2.5">
              <div class="flex min-w-0 flex-col gap-0.5">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="text-sm font-medium">{agent.name}</span>
                  <span class="text-muted-foreground font-mono text-xs">{agent.id}</span>
                  {#if agent.version !== "-"}
                    <span class="text-muted-foreground text-xs">v{agent.version}</span>
                  {/if}
                  <Badge variant="secondary" class="text-[10px]">未導入</Badge>
                </div>
                {#if agent.description}
                  <p class="text-muted-foreground text-xs">
                    {agent.description}
                  </p>
                {/if}
              </div>
              <div class="flex flex-wrap items-center gap-1.5">
                {#if agent.installable}
                  <Button
                    size="sm"
                    disabled={busy || nodeId === ""}
                    onclick={() => void installAgent(agent, true)}
                  >
                    <DownloadIcon />
                    インストール
                  </Button>
                {/if}
                {#if agent.package_available}
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy || nodeId === ""}
                    onclick={() => void setPackageEnabled(agent, true)}
                    title="npx/uvx は実行時にパッケージを取得します (実体は保存しません)"
                  >
                    <PackageIcon />
                    npx/uvx を有効化
                  </Button>
                {/if}
                {#if !agent.installable && !agent.package_available}
                  <span class="text-muted-foreground text-[11px]">
                    このノードでは利用できません
                  </span>
                {/if}
              </div>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </Dialog.Content>
</Dialog.Root>

<!-- カスタムエージェント定義ダイアログ -->
<Dialog.Root open={customOpen} onOpenChange={(open) => (customOpen = open)}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {customEditId === null ? "カスタムエージェントを追加" : `${customEditId} を編集`}
      </Dialog.Title>
      <Dialog.Description>
        ACP 対応の任意のコマンドをエージェントとして登録します (<code
          >~/.flexagent/custom_agents.json</code
        > に保存)。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-3" onsubmit={submitCustom}>
      <div class="grid gap-1.5">
        <Label for="custom-id">エージェントID</Label>
        <Input
          id="custom-id"
          bind:value={customForm.id}
          disabled={customEditId !== null}
          placeholder="my-agent"
          class="font-mono text-xs"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-name">表示名 (任意)</Label>
        <Input
          id="custom-name"
          bind:value={customForm.name}
          placeholder="My Agent"
          class="text-xs"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-command">実行コマンド</Label>
        <Input
          id="custom-command"
          bind:value={customForm.command}
          placeholder="npx / node / /path/to/agent"
          class="font-mono text-xs"
        />
        <p class="text-muted-foreground text-[11px]">
          PATH 上のコマンド名、または絶対パス。ACP を stdio で話す実行ファイルを指定します。
        </p>
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-args">引数 (空白区切り)</Label>
        <Input
          id="custom-args"
          bind:value={customForm.args}
          placeholder="-y @scope/agent-acp"
          class="font-mono text-xs"
        />
      </div>
      <div class="grid gap-1.5">
        <Label for="custom-env">環境変数 (1行1件 <code>KEY=VALUE</code>)</Label>
        <textarea
          id="custom-env"
          bind:value={customForm.env}
          rows="3"
          class="border-input bg-background focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-xs outline-none focus-visible:ring-1"
          placeholder="LOG_LEVEL=info"></textarea>
      </div>
      <Dialog.Footer>
        <Dialog.Close>キャンセル</Dialog.Close>
        <Button type="submit" disabled={customBusy}>
          {customBusy ? "保存中…" : "保存"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
