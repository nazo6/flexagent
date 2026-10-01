<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import AppSidebar from '$lib/components/layout/AppSidebar.svelte';
  import ConnectionMenu from '$lib/components/ConnectionMenu.svelte';
  import TokenDialog from '$lib/components/TokenDialog.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Sheet from '$lib/components/ui/sheet';
  import { Toaster } from '$lib/components/ui/sonner';
  import { connection, initApp } from '$lib/stores/app.svelte';
  import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
  import MenuIcon from '@lucide/svelte/icons/menu';
  import PlugZapIcon from '@lucide/svelte/icons/plug-zap';
  import PlusIcon from '@lucide/svelte/icons/plus';

  let { children } = $props();

  let mobileDrawerOpen = $state(false);

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
  <div class="bg-background text-foreground flex h-svh w-full overflow-hidden">
    <!-- デスクトップ用サイドバー (md以上で常時表示) -->
    <div class="hidden md:flex md:w-64 lg:w-72 md:shrink-0 md:h-full">
      <AppSidebar />
    </div>

    <!-- モバイル用ドロワー (Sheet) -->
    <Sheet.Root bind:open={mobileDrawerOpen}>
      <Sheet.Content side="left" class="p-0 w-80 max-w-[85vw]">
        <AppSidebar onNavigate={() => (mobileDrawerOpen = false)} />
      </Sheet.Content>
    </Sheet.Root>

    <!-- メインエリア -->
    <div class="flex flex-1 flex-col h-full overflow-hidden">
      <!-- モバイル用上部バー (md未満で表示) -->
      <header class="bg-background/80 flex items-center justify-between border-b px-3 py-2 md:hidden shrink-0">
        <button
          type="button"
          class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
          onclick={() => (mobileDrawerOpen = true)}
          aria-label="メニューを開く"
        >
          <MenuIcon class="size-4" />
        </button>

        <a href="/" class="flex items-center gap-1.5 font-semibold text-sm">
          <div class="bg-primary text-primary-foreground flex size-5 items-center justify-center rounded font-mono text-[10px] font-bold">
            fxg
          </div>
          <span>FlexAgent</span>
        </a>

        <a
          href="/"
          class="hover:bg-accent flex size-8 items-center justify-center rounded-md border text-muted-foreground"
          aria-label="新規セッション"
        >
          <PlusIcon class="size-4" />
        </a>
      </header>

      <!-- ページコンテンツ (フルハイト) -->
      <main class="flex-1 overflow-y-auto">
        {@render children()}
      </main>
    </div>
  </div>
{/if}
