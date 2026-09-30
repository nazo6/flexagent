<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import { sync } from '$lib/stores/app.svelte';
  import { toast } from 'svelte-sonner';
  import OctagonXIcon from '@lucide/svelte/icons/octagon-x';

  let busy = $state(false);

  async function confirmKill() {
    if (busy) return;
    busy = true;
    try {
      const nodes = await sync.killSwitch('web ui kill switch');
      toast.success(`緊急停止を送信しました (${nodes} ノードへ配信)`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }
</script>

<AlertDialog.Root>
  <AlertDialog.Trigger>
    {#snippet child({ props })}
      <Button variant="destructive" size="sm" {...props}>
        <OctagonXIcon />
        <span class="hidden sm:inline">緊急停止</span>
      </Button>
    {/snippet}
  </AlertDialog.Trigger>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>全セッションを緊急停止しますか？</AlertDialog.Title>
      <AlertDialog.Description>
        全ノードで稼働中のセッション・実行中プロセスツリー・PTY
        を即時強制停止します。この操作は監査ログに記録されます。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={confirmKill} disabled={busy}>
        {busy ? '送信中…' : '停止する'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
