<script lang="ts">
  import { onMount } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { connection, logoutAndStop } from '$lib/stores/app.svelte';
  import {
    disablePushNotifications,
    enablePushNotifications,
    pushCapability,
    type PushCapability
  } from '$lib/push';
  import { toast } from 'svelte-sonner';
  import BellIcon from '@lucide/svelte/icons/bell';
  import BellOffIcon from '@lucide/svelte/icons/bell-off';
  import CableIcon from '@lucide/svelte/icons/cable';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import LogOutIcon from '@lucide/svelte/icons/log-out';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import ServerIcon from '@lucide/svelte/icons/server';
  import { cn } from '$lib/utils';

  interface Props {
    class?: string;
    align?: 'start' | 'center' | 'end';
  }

  let { class: className, align = 'start' }: Props = $props();

  let addOpen = $state(false);
  let newLabel = $state('');
  let newUrl = $state('');
  let push = $state<PushCapability | null>(null);
  let pushBusy = $state(false);

  const roleLabel = $derived(
    connection.systemInfo?.role === 'central_server' ? '中央サーバー' : 'ローカルノード'
  );
  const originLabel = $derived(connection.origin || '(同一オリジン)');

  // Push 対応状況は接続先バックエンドに依存するため初回マウント時に確認する
  onMount(() => {
    void refreshPush();
  });

  async function refreshPush() {
    try {
      push = await pushCapability();
    } catch {
      push = null;
    }
  }

  async function togglePush() {
    if (push === null || pushBusy) return;
    pushBusy = true;
    try {
      if (push.subscribed) {
        await disablePushNotifications();
        toast('Push 通知を解除しました');
      } else {
        await enablePushNotifications();
        toast.success('Push 通知を有効化しました');
      }
      await refreshPush();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pushBusy = false;
    }
  }

  function addTarget(event: SubmitEvent) {
    event.preventDefault();
    if (!newUrl.trim()) return;
    try {
      const parsed = new URL(newUrl.trim());
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
        throw new Error('http:// または https:// の URL を指定してください');
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'URL が不正です');
      return;
    }
    connection.addTarget(newLabel, newUrl);
    addOpen = false;
    newLabel = '';
    newUrl = '';
    toast.success('切替先を追加しました');
  }

  async function logout() {
    try {
      await logoutAndStop();
      toast('ログアウトしました');
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger>
    {#snippet child({ props })}
      <Button
        variant="outline"
        size="sm"
        {...props}
        class={cn('gap-1.5 text-xs', className)}
        title={`現在の接続先: ${roleLabel} (${originLabel})`}
      >
        <div class="flex items-center gap-1.5 min-w-0">
          <ServerIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate">{roleLabel}</span>
        </div>
        <ChevronDownIcon class="size-3 text-muted-foreground/70 shrink-0 ml-auto" />
      </Button>
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content {align} class="w-80">
    <DropdownMenu.Label>現在の接続先</DropdownMenu.Label>
    <DropdownMenu.Item disabled>
      <span class="truncate">{originLabel}</span>
      <span class="text-muted-foreground ml-auto text-xs">{roleLabel}</span>
    </DropdownMenu.Item>
    <DropdownMenu.Separator />
    {#if connection.availableTargets.length > 0}
      <DropdownMenu.Label>切替先 (そのオリジンの UI へ移動)</DropdownMenu.Label>
      {#each connection.availableTargets as target (target.id)}
        <DropdownMenu.Item onSelect={() => connection.switchTo(target.id)}>
          <CableIcon />
          <span class="flex-1 truncate">{target.label}</span>
          <span class="text-muted-foreground text-xs">{target.url}</span>
        </DropdownMenu.Item>
      {/each}
      <DropdownMenu.Separator />
    {/if}
    <DropdownMenu.Item onSelect={() => (addOpen = true)}>
      <PlusIcon />
      切替先を追加… (例: http://localhost:7860)
    </DropdownMenu.Item>
    {#if push !== null && push.supported && push.serverEnabled}
      <DropdownMenu.Separator />
      <DropdownMenu.Item disabled={pushBusy} onSelect={() => void togglePush()}>
        {#if push.subscribed}
          <BellOffIcon />
          Push 通知を解除
        {:else}
          <BellIcon />
          Push 通知を有効化 (承認バナー)
        {/if}
      </DropdownMenu.Item>
    {/if}
    <DropdownMenu.Item onSelect={logout}>
      <LogOutIcon />
      ログアウト
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<Dialog.Root bind:open={addOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>切替先を追加</Dialog.Title>
      <Dialog.Description>
        中央サーバーまたはノードの Web UI の URL
        を登録します。切替時はそのオリジンの UI へ移動します (トークンはオリジンごとに
        独立して保持されます)。
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={addTarget}>
      <div class="grid gap-2">
        <Label for="target-label">表示名</Label>
        <Input id="target-label" bind:value={newLabel} placeholder="自宅サーバー" />
      </div>
      <div class="grid gap-2">
        <Label for="target-url">URL</Label>
        <Input
          id="target-url"
          bind:value={newUrl}
          placeholder="http://localhost:7860 または https://fxg.example.ts.net"
        />
      </div>
      <Dialog.Footer>
        <Button type="submit" disabled={newUrl.trim() === ''}>追加</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
