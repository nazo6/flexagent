<script lang="ts">
  import { page } from '$app/state';

  const sessionId = $derived(page.params.id);
</script>

<p class="text-muted-foreground text-sm">
  セッション詳細 ({sessionId}) は次のコミットで実装します。
</p>
