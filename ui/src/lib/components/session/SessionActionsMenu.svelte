<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { SessionSummary } from '$lib/generated/SessionSummary';
  import { sync } from '$lib/stores/app.svelte';
  import { cn } from '$lib/utils';
  import { toast } from 'svelte-sonner';
  import ArchiveIcon from '@lucide/svelte/icons/archive';
  import ArchiveRestoreIcon from '@lucide/svelte/icons/archive-restore';
  import MoreHorizontalIcon from '@lucide/svelte/icons/more-horizontal';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';

  interface Props {
    session: SessionSummary;
    /** 削除完了後に呼ばれる (開いているセッションからの遷移など)。 */
    onDeleted?: () => void;
    /** トリガーボタンの追加クラス (ホバー表示など)。 */
    buttonClass?: string;
    /** トリガーアイコンのサイズクラス。 */
    iconClass?: string;
    align?: 'start' | 'center' | 'end';
  }

  let { session, onDeleted, buttonClass, iconClass = 'size-3.5', align = 'end' }: Props = $props();

  const archived = $derived(session.archived_at !== null);
  let confirmOpen = $state(false);
  let busy = $state(false);

  /** アーカイブ/復元する (可逆。イベントログは保持される)。 */
  async function toggleArchive() {
    if (busy) return;
    busy = true;
    try {
      await sync.archiveSession(session.session_id, !archived);
      toast.success(archived ? 'アーカイブを解除しました' : 'アーカイブしました');
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }

  /** 削除する (会話ログを消去。復元不能)。 */
  async function handleDelete() {
    if (busy) return;
    busy = true;
    try {
      await sync.deleteSession(session.session_id);
      toast.success('セッションを削除しました');
      confirmOpen = false;
      onDeleted?.();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }
</script>

<!-- サイドバーの行 (リンク) 内でもクリックが遷移しないよう伝播を止める -->
<span
  class="inline-flex"
  role="presentation"
  onclick={(event) => {
    event.preventDefault();
    event.stopPropagation();
  }}
>
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class={cn(
        'text-muted-foreground hover:text-foreground focus-visible:ring-ring/40 inline-flex items-center justify-center rounded p-0.5 outline-none transition-colors focus-visible:ring-1',
        buttonClass
      )}
      title="セッション操作 (アーカイブ / 削除)"
    >
      <MoreHorizontalIcon class={iconClass} />
    </DropdownMenu.Trigger>
    <DropdownMenu.Content {align} class="w-44">
      <DropdownMenu.Item disabled={busy} onSelect={() => void toggleArchive()}>
        {#if archived}
          <ArchiveRestoreIcon />
          アーカイブを解除
        {:else}
          <ArchiveIcon />
          アーカイブ
        {/if}
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item
        variant="destructive"
        disabled={busy}
        onSelect={() => (confirmOpen = true)}
      >
        <Trash2Icon />
        削除…
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
</span>

<!-- 削除の確認ダイアログ -->
<AlertDialog.Root bind:open={confirmOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title class="flex items-center gap-2 text-destructive">
        <Trash2Icon class="size-5" />
        セッションを削除しますか？
      </AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        セッション「{session.title}」の会話ログ・承認履歴・ツール実行履歴をすべて削除します。
        この操作は取り消せません (非表示にするだけのアーカイブは復元できます)。
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={busy}>キャンセル</AlertDialog.Cancel>
      <AlertDialog.Action
        class="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
        disabled={busy}
        onclick={handleDelete}
      >
        {busy ? '削除中…' : '削除'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
