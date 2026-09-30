<script lang="ts">
  import { page } from '$app/state';
  import { Badge } from '$lib/components/ui/badge';
  import { cn } from '$lib/utils';
  import { sync } from '$lib/stores/app.svelte';
  import ConnectionMenu from './ConnectionMenu.svelte';
  import KillSwitchButton from './KillSwitchButton.svelte';
  import SyncStatusBadge from './SyncStatusBadge.svelte';

  const navItems = [
    { href: '/', label: 'セッション' },
    { href: '/inbox', label: '承認 Inbox' },
    { href: '/projects', label: 'プロジェクト' },
    { href: '/audit', label: '監査ログ' },
    { href: '/search', label: '検索' }
  ] as const;

  const currentPath = $derived(page.url.pathname);

  function isActive(href: string): boolean {
    return href === '/' ? currentPath === '/' : currentPath.startsWith(href);
  }
</script>

<header class="bg-background/80 sticky top-0 z-40 border-b backdrop-blur">
  <div class="mx-auto flex max-w-7xl flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2">
    <a href="/" class="shrink-0 text-sm font-semibold tracking-tight">
      fxg<span class="text-muted-foreground font-normal"> FlexAgent</span>
    </a>

    <nav class="scrollbar-none flex items-center gap-0.5 overflow-x-auto text-sm">
      {#each navItems as item (item.href)}
        <a
          href={item.href}
          class={cn(
            'flex items-center gap-1.5 rounded-md px-2.5 py-1.5 whitespace-nowrap transition-colors',
            isActive(item.href)
              ? 'bg-accent text-accent-foreground font-medium'
              : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
          )}
        >
          {item.label}
          {#if item.href === '/inbox' && sync.inbox.length > 0}
            <Badge variant="destructive">{sync.inbox.length}</Badge>
          {/if}
        </a>
      {/each}
    </nav>

    <div class="ml-auto flex items-center gap-2">
      <SyncStatusBadge />
      <ConnectionMenu />
      <KillSwitchButton />
    </div>
  </div>
</header>
