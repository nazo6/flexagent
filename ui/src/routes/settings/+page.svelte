<script lang="ts">
  import { onMount } from 'svelte';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Card } from '$lib/components/ui/card';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import type { IssueNodeTokenResponse } from '$lib/generated/IssueNodeTokenResponse';
  import type { NodeTokenSummary } from '$lib/generated/NodeTokenSummary';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import KeyRoundIcon from '@lucide/svelte/icons/key-round';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import ShieldIcon from '@lucide/svelte/icons/shield';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  const isServer = $derived(sync.connection.roleHint === 'central_server');

  // --- ノードペアリング (中央サーバーのみ) ---
  let tokens = $state<NodeTokenSummary[]>([]);
  let loadingTokens = $state(false);

  // 新規ノードトークン発行
  let issueOpen = $state(false);
  let issueNodeId = $state('');
  let issueBusy = $state(false);
  /** 発行直後の平文トークン (このダイアログを閉じると再表示できない)。 */
  let issued = $state<IssueNodeTokenResponse | null>(null);

  // トークン失効確認
  let revokeTarget = $state<string | null>(null);
  let revokeBusy = $state(false);

  // --- クライアント認証トークン ---
  let rotateOpen = $state(false);
  let rotateBusy = $state(false);

  onMount(() => {
    void loadTokens();
  });

  async function loadTokens(): Promise<void> {
    if (!isServer || loadingTokens) return;
    loadingTokens = true;
    try {
      tokens = await sync.connection.client.nodeTokens();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      loadingTokens = false;
    }
  }

  async function submitIssue(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (issueBusy || issueNodeId.trim() === '') return;
    issueBusy = true;
    try {
      issued = await sync.connection.client.issueNodeToken({ node_id: issueNodeId.trim() });
      issueOpen = false;
      issueNodeId = '';
      await loadTokens();
      await sync.refreshNodes();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      issueBusy = false;
    }
  }

  async function confirmRevoke(): Promise<void> {
    const nodeId = revokeTarget;
    if (nodeId === null || revokeBusy) return;
    revokeBusy = true;
    try {
      await sync.connection.client.revokeNodeToken(nodeId);
      toast.success(`${nodeId} のノードトークンを失効しました`);
      revokeTarget = null;
      await loadTokens();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      revokeBusy = false;
    }
  }

  async function copyText(text: string, label: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(`${label}をコピーしました`);
    } catch {
      toast.error('クリップボードへコピーできませんでした');
    }
  }

  async function confirmRotate(): Promise<void> {
    if (rotateBusy) return;
    rotateBusy = true;
    try {
      const result = await sync.connection.client.rotateAuthToken();
      // 旧トークンは即時無効化されるため、新トークンで即座に再認証する
      await sync.connection.login(result.token);
      sync.restart();
      toast.success('クライアント認証トークンを再生成しました');
      rotateOpen = false;
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      rotateBusy = false;
    }
  }
</script>

<div class="mx-auto flex max-w-3xl flex-col gap-4 p-4 md:p-6">
  <div>
    <h1 class="text-xl font-semibold">設定</h1>
    <p class="text-muted-foreground text-sm">認証トークンとノードペアリングの管理</p>
  </div>

  <!-- クライアント認証トークン -->
  <Card class="gap-3 p-4">
    <div class="flex items-start gap-2">
      <ShieldIcon class="text-muted-foreground mt-0.5 size-4" />
      <div class="flex flex-1 flex-col gap-1">
        <h2 class="font-medium">クライアント認証トークン</h2>
        <p class="text-muted-foreground text-xs">
          Web UI / CLI がこのバックエンドへ接続するための共通トークンです。再生成すると
          旧トークン (Bearer / Cookie) は即時無効化され、他端末は再ログインが必要になります。
          トークンの平文は表示できません (<code>~/.flexagent/auth_token</code> を参照)。
        </p>
        <div class="mt-1">
          <Button variant="outline" size="sm" onclick={() => (rotateOpen = true)}>
            <RefreshCwIcon />
            トークンを再生成
          </Button>
        </div>
      </div>
    </div>
  </Card>

  <!-- ノードペアリング (中央サーバーのみ) -->
  <Card class="gap-3 p-4">
    <div class="flex items-start gap-2">
      <KeyRoundIcon class="text-muted-foreground mt-0.5 size-4" />
      <div class="flex flex-1 flex-col gap-2">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h2 class="font-medium">ノードペアリング</h2>
            <p class="text-muted-foreground text-xs">
              <code>fxg daemon</code> を中央サーバーへ接続するためのノード個別トークン
              (<code>~/.flexagent/node_token</code>) を発行・失効します。
            </p>
          </div>
          <div class="flex items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              disabled={!isServer || loadingTokens}
              onclick={() => void loadTokens()}
            >
              <RefreshCwIcon />
              再読込
            </Button>
            <Button size="sm" disabled={!isServer} onclick={() => (issueOpen = true)}>
              トークン発行
            </Button>
          </div>
        </div>

        {#if !isServer}
          <p class="text-muted-foreground text-xs">
            ノードトークンの管理は中央サーバー接続時のみ利用できます。現在はローカルノード
            (<code>{sync.connection.origin}</code>) に接続しています。
          </p>
        {:else if tokens.length === 0}
          <p class="text-muted-foreground text-xs">
            {loadingTokens ? '読み込み中…' : '発行済みのノードトークンはありません。'}
          </p>
        {:else}
          <ul class="flex list-none flex-col gap-1 p-0 text-xs">
            {#each tokens as token (token.node_id)}
              <li
                class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5"
              >
                <span class="font-medium">{token.node_id}</span>
                <span class="text-muted-foreground font-mono">{token.token_prefix}…</span>
                <Badge variant="outline" class="text-[10px]">発行済み</Badge>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  class="text-muted-foreground hover:text-destructive ml-auto"
                  disabled={revokeBusy}
                  onclick={() => (revokeTarget = token.node_id)}
                  title="トークンを失効"
                >
                  <Trash2Icon />
                  <span class="sr-only">{token.node_id} のトークンを失効</span>
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  </Card>
</div>

<!-- ノードトークン発行 -->
<Dialog.Root open={issueOpen} onOpenChange={(open) => (issueOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>ノードトークンを発行</Dialog.Title>
      <Dialog.Description>
        ノードID (<code>fxg daemon</code> の <code>node_id</code>) を入力してください。
        既存トークンがある場合は上書き (再発行) されます。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitIssue}>
      <div class="grid gap-2">
        <Label for="node-id">ノードID</Label>
        <Input id="node-id" bind:value={issueNodeId} placeholder="home-win" class="font-mono text-xs" />
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (issueOpen = false)}>
          キャンセル
        </Button>
        <Button type="submit" disabled={issueBusy || issueNodeId.trim() === ''}>
          {issueBusy ? '発行中…' : '発行'}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- 発行された平文トークンの表示 (一度きり) -->
<Dialog.Root open={issued !== null} onOpenChange={(open) => !open && (issued = null)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>ノードトークンを発行しました</Dialog.Title>
      <Dialog.Description>
        このトークンは一度きりの表示です (サーバーにはハッシュのみ保存)。ノード側の
        <code>~/.flexagent/node_token</code> に保存してください。
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3">
      <div class="grid gap-2">
        <Label>ノードID</Label>
        <code class="bg-muted rounded-md border px-2 py-1 text-xs">{issued?.node_id}</code>
      </div>
      <div class="grid gap-2">
        <Label>トークン</Label>
        <div class="flex items-center gap-1.5">
          <code class="bg-muted min-w-0 flex-1 truncate rounded-md border px-2 py-1 font-mono text-xs">
            {issued?.token ?? ''}
          </code>
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
            onclick={() => issued && void copyText(issued.token, 'トークン')}
          >
            <CopyIcon class="size-3.5" />
            コピー
          </Button>
        </div>
      </div>
      <pre
        class="bg-muted overflow-x-auto rounded-md border p-2 text-[11px] leading-relaxed">{`# ノード側 (保存後に fxg daemon を再起動)
echo "${issued?.token ?? ''}" > ~/.flexagent/node_token`}</pre>
    </div>
    <Dialog.Footer>
      <Button
        type="button"
        variant="outline"
        onclick={() => issued && void copyText(issued.token, 'トークン')}
      >
        <CopyIcon />
        トークンをコピー
      </Button>
      <Button type="button" onclick={() => (issued = null)}>閉じる</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- ノードトークン失効確認 -->
<AlertDialog.Root
  open={revokeTarget !== null}
  onOpenChange={(open) => !open && (revokeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>ノードトークンを失効しますか？</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        <code>{revokeTarget}</code> のトークンを失効します。接続中の該当ノードは切断され、
        再ペアリングするまで中央サーバーへ接続できなくなります。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={revokeBusy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" disabled={revokeBusy} onclick={() => void confirmRevoke()}>
        {revokeBusy ? '失効中…' : '失効'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- クライアントトークン再生成確認 -->
<AlertDialog.Root open={rotateOpen} onOpenChange={(open) => (rotateOpen = open)}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>クライアント認証トークンを再生成しますか？</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        新しいトークンを生成し、旧トークンを即時無効化します。このブラウザは自動で再認証
        されますが、他の端末 (CLI / PWA / Android) は新しいトークンの入力が必要になります。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={rotateBusy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action disabled={rotateBusy} onclick={() => void confirmRotate()}>
        {rotateBusy ? '再生成中…' : '再生成'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
