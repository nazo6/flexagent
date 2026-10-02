<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Label } from '$lib/components/ui/label';
  import { connection } from '$lib/stores/app.svelte';
  import { loginAndStart } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import KeyRoundIcon from '@lucide/svelte/icons/key-round';

  let token = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    error = null;
    try {
      await loginAndStart(token);
      token = '';
      toast.success('接続しました');
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root open={connection.tokenRequired}>
  <Dialog.Content
    class="sm:max-w-md"
    showCloseButton={false}
    onInteractOutside={(event) => event.preventDefault()}
    onEscapeKeydown={(event) => event.preventDefault()}
  >
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <KeyRoundIcon class="size-4" />
        認証トークンを入力
      </Dialog.Title>
      <Dialog.Description>
        {#if connection.role === 'central_server'}
          中央サーバー
        {:else}
          ローカルノード
        {/if}
        の認証トークンを入力してください。成功すると <code>fxg_session</code> Cookie
        が保存され、以降は自動で認証されます。
      </Dialog.Description>
    </Dialog.Header>

    <form class="flex flex-col gap-4" onsubmit={submit}>
      <div class="grid gap-2">
        <Label for="fxg-token">トークン</Label>
        <Input
          id="fxg-token"
          type="password"
          autocomplete="off"
          placeholder={connection.role === 'central_server'
            ? 'サーバーの auth_token の内容'
            : '~/.flexagent/auth_token の内容'}
          bind:value={token}
          disabled={busy}
        />
        <p class="text-muted-foreground text-xs">
          {#if connection.role === 'central_server'}
            サーバー上の <code>auth_token</code> ファイル（既定:{' '}
            <code>~/.flexagent/auth_token</code>）を確認してください。Docker
            の場合は <code>docker compose exec server cat /data/auth_token</code>{' '}
            で確認できます。
          {:else}
            ノード上の <code>~/.flexagent/auth_token</code>、または{' '}
            <code>fxg auth token</code> で確認できます（Docker の場合は{' '}
            <code>docker compose exec node fxg auth token</code>）。
          {/if}
        </p>
      </div>

      {#if error}
        <p class="text-destructive text-sm">{error}</p>
      {/if}

      <Button type="submit" disabled={busy || token.trim() === ''}>
        {busy ? '確認中…' : '接続'}
      </Button>
    </form>
  </Dialog.Content>
</Dialog.Root>
