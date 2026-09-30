<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import AppHeader from '$lib/components/AppHeader.svelte';
  import ConnectionMenu from '$lib/components/ConnectionMenu.svelte';
  import TokenDialog from '$lib/components/TokenDialog.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Toaster } from '$lib/components/ui/sonner';
  import { connection, initApp } from '$lib/stores/app.svelte';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import PlugZapIcon from '@lucide/svelte/icons/plug-zap';

  let { children } = $props();

  onMount(() => {
    void initApp();
  });

  async function retry() {
    await initApp();
  }
</script>

<Toaster richColors position="top-center" />
<TokenDialog />

{#if !connection.ready}
  <div
    class="bg-background text-muted-foreground flex min-h-svh items-center justify-center gap-2"
  >
    <LoaderCircleIcon class="size-4 animate-spin" />
    接続を確認しています…
  </div>
{:else if connection.error !== null}
  <div class="bg-background flex min-h-svh items-center justify-center p-6">
    <div class="flex max-w-md flex-col items-center gap-3 text-center">
      <PlugZapIcon class="text-destructive size-8" />
      <h1 class="text-lg font-semibold">サーバーに接続できません</h1>
      <p class="text-muted-foreground text-sm">{connection.error}</p>
      <p class="text-muted-foreground text-xs">
        中央サーバーがダウンしている場合は、下のメニューからローカルノード
        (例: <code>http://localhost:7860</code>) へ接続先を切り替えられます。
      </p>
      <div class="flex items-center gap-2">
        <Button variant="outline" onclick={retry}>再試行</Button>
        <ConnectionMenu />
      </div>
    </div>
  </div>
{:else if connection.tokenRequired}
  <div class="bg-background min-h-svh"></div>
{:else}
  <div class="bg-background text-foreground flex min-h-svh flex-col">
    <AppHeader />
    <main class="mx-auto w-full max-w-7xl flex-1 px-3 py-4">
      {@render children()}
    </main>
  </div>
{/if}
