<script lang="ts">
  import { page } from '$app/state';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { formatRelativeTimeCompact } from '$lib/format';
  import { getNodeAvailability } from '$lib/node-status';
  import { pinned } from '$lib/stores/pinned.svelte';
  import { sync } from '$lib/stores/app.svelte';
  import { cn } from '$lib/utils';
  import ConnectionMenu from '$lib/components/ConnectionMenu.svelte';
  import KillSwitchButton from '$lib/components/KillSwitchButton.svelte';
  import SyncStatusBadge from '$lib/components/SyncStatusBadge.svelte';
  import type { SessionSummary } from '$lib/generated/SessionSummary';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import FolderGit2Icon from '@lucide/svelte/icons/folder-git-2';
  import InboxIcon from '@lucide/svelte/icons/inbox';
  import PinIcon from '@lucide/svelte/icons/pin';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import SearchIcon from '@lucide/svelte/icons/search';
  import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert';

  interface Props {
    onNavigate?: () => void;
  }

  let { onNavigate }: Props = $props();

  let searchQuery = $state('');
  let collapsedSections = $state<Record<string, boolean>>({});

  const currentPath = $derived(page.url.pathname);
  const activeSessionId = $derived(
    currentPath.startsWith('/sessions/') ? currentPath.replace('/sessions/', '') : null
  );

  function toggleSection(sectionKey: string) {
    collapsedSections[sectionKey] = !collapsedSections[sectionKey];
  }

  function matchesSearch(session: SessionSummary, query: string): boolean {
    if (!query) return true;
    const q = query.toLowerCase();
    return (
      session.title.toLowerCase().includes(q) ||
      session.project_id.toLowerCase().includes(q) ||
      (session.git_branch?.toLowerCase().includes(q) ?? false) ||
      session.agent_id.toLowerCase().includes(q)
    );
  }

  const filteredSessions = $derived(
    sync.sessions.filter((session) => matchesSearch(session, searchQuery.trim()))
  );

  const pendingSessionIds = $derived(
    new Set(sync.inbox.map((req) => req.session_id))
  );

  const needsAttentionSessions = $derived(
    filteredSessions.filter(
      (s) => pendingSessionIds.has(s.session_id) || s.status === 'error'
    )
  );

  const workingSessions = $derived(
    filteredSessions.filter(
      (s) =>
        s.status === 'running' &&
        !pendingSessionIds.has(s.session_id)
    )
  );

  const pinnedSessions = $derived(
    filteredSessions.filter(
      (s) =>
        pinned.isPinned(s.session_id) &&
        !needsAttentionSessions.some((item) => item.session_id === s.session_id) &&
        !workingSessions.some((item) => item.session_id === s.session_id)
    )
  );

  const recentSessions = $derived(
    filteredSessions.filter(
      (s) =>
        !needsAttentionSessions.some((item) => item.session_id === s.session_id) &&
        !workingSessions.some((item) => item.session_id === s.session_id) &&
        !pinnedSessions.some((item) => item.session_id === s.session_id)
    )
  );

  function getNode(nodeId: string) {
    return sync.nodes.find((n) => n.node_id === nodeId);
  }

  function handleLinkClick() {
    onNavigate?.();
  }
</script>

<aside class="bg-card flex h-full w-full flex-col border-r">
  <div class="flex items-center justify-between border-b px-3 py-2.5">
    <a
      href="/"
      class="flex items-center gap-2 font-semibold tracking-tight hover:opacity-80 transition-opacity"
      onclick={handleLinkClick}
    >
      <div class="bg-primary text-primary-foreground flex size-6 items-center justify-center rounded font-mono text-xs font-bold">
        fxg
      </div>
      <span class="text-sm">FlexAgent</span>
    </a>

    <a
      href="/"
      onclick={handleLinkClick}
      title="新規セッション"
      class="hover:bg-accent text-muted-foreground hover:text-foreground flex size-7 items-center justify-center rounded-md border"
    >
      <PlusIcon class="size-4" />
    </a>
  </div>

  <nav class="flex flex-col gap-0.5 border-b p-2 text-xs">
    <a
      href="/"
      onclick={handleLinkClick}
      class={cn(
        'flex items-center justify-between rounded-md px-2.5 py-1.5 font-medium transition-colors',
        currentPath === '/'
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
      )}
    >
      <div class="flex items-center gap-2">
        <PlusIcon class="size-3.5" />
        <span>新規セッション</span>
      </div>
    </a>

    <a
      href="/inbox"
      onclick={handleLinkClick}
      class={cn(
        'flex items-center justify-between rounded-md px-2.5 py-1.5 font-medium transition-colors',
        currentPath.startsWith('/inbox')
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
      )}
    >
      <div class="flex items-center gap-2">
        <InboxIcon class="size-3.5" />
        <span>承認 Inbox</span>
      </div>
      {#if sync.inbox.length > 0}
        <Badge variant="destructive" class="px-1.5 py-0 text-[10px] leading-tight">
          {sync.inbox.length}
        </Badge>
      {/if}
    </a>

    <a
      href="/projects"
      onclick={handleLinkClick}
      class={cn(
        'flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors',
        currentPath.startsWith('/projects')
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
      )}
    >
      <FolderGit2Icon class="size-3.5" />
      <span>プロジェクト</span>
    </a>

    <a
      href="/search"
      onclick={handleLinkClick}
      class={cn(
        'flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors',
        currentPath.startsWith('/search')
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
      )}
    >
      <SearchIcon class="size-3.5" />
      <span>検索</span>
    </a>

    <a
      href="/audit"
      onclick={handleLinkClick}
      class={cn(
        'flex items-center gap-2 rounded-md px-2.5 py-1.5 font-medium transition-colors',
        currentPath.startsWith('/audit')
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground'
      )}
    >
      <ShieldAlertIcon class="size-3.5" />
      <span>監査ログ</span>
    </a>
  </nav>

  <div class="border-b px-2 py-2">
    <div class="relative flex items-center">
      <SearchIcon class="text-muted-foreground absolute left-2 size-3.5 pointer-events-none" />
      <input
        type="text"
        bind:value={searchQuery}
        placeholder="セッションを絞り込み…"
        class="bg-muted/40 placeholder:text-muted-foreground focus-visible:ring-ring/40 w-full rounded-md border py-1 pr-2 pl-7 text-xs outline-none focus-visible:ring-1"
      />
    </div>
  </div>

  <div class="scrollbar-thin flex-1 overflow-y-auto p-2">
    {#if filteredSessions.length === 0}
      <div class="text-muted-foreground p-3 text-center text-xs">
        {searchQuery ? '該当するセッションがありません' : 'セッションがまだありません'}
      </div>
    {:else}
      {#if needsAttentionSessions.length > 0}
        <div class="mb-3">
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-1 text-[11px] font-semibold tracking-wider uppercase select-none"
            onclick={() => toggleSection('attention')}
          >
            <span class="text-destructive flex items-center gap-1.5">
              <span>NEEDS ATTENTION</span>
              <span class="rounded-full bg-destructive/15 px-1.5 py-0.2 text-[10px] text-destructive">
                {needsAttentionSessions.length}
              </span>
            </span>
            {#if collapsedSections['attention']}
              <ChevronRightIcon class="size-3" />
            {:else}
              <ChevronDownIcon class="size-3" />
            {/if}
          </button>
          {#if !collapsedSections['attention']}
            <ul class="flex list-none flex-col gap-0.5 p-0">
              {#each needsAttentionSessions as session (session.session_id)}
                {@render sessionRow(session)}
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      {#if workingSessions.length > 0}
        <div class="mb-3">
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-1 text-[11px] font-semibold tracking-wider uppercase select-none"
            onclick={() => toggleSection('working')}
          >
            <span class="flex items-center gap-1.5">
              <span>WORKING</span>
              <span class="rounded-full bg-emerald-500/15 px-1.5 py-0.2 text-[10px] text-emerald-600 dark:text-emerald-400">
                {workingSessions.length}
              </span>
            </span>
            {#if collapsedSections['working']}
              <ChevronRightIcon class="size-3" />
            {:else}
              <ChevronDownIcon class="size-3" />
            {/if}
          </button>
          {#if !collapsedSections['working']}
            <ul class="flex list-none flex-col gap-0.5 p-0">
              {#each workingSessions as session (session.session_id)}
                {@render sessionRow(session)}
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      {#if pinnedSessions.length > 0}
        <div class="mb-3">
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-1 text-[11px] font-semibold tracking-wider uppercase select-none"
            onclick={() => toggleSection('pinned')}
          >
            <span>PINNED</span>
            {#if collapsedSections['pinned']}
              <ChevronRightIcon class="size-3" />
            {:else}
              <ChevronDownIcon class="size-3" />
            {/if}
          </button>
          {#if !collapsedSections['pinned']}
            <ul class="flex list-none flex-col gap-0.5 p-0">
              {#each pinnedSessions as session (session.session_id)}
                {@render sessionRow(session)}
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      {#if recentSessions.length > 0}
        <div class="mb-3">
          <button
            type="button"
            class="text-muted-foreground hover:text-foreground flex w-full items-center justify-between px-1 py-1 text-[11px] font-semibold tracking-wider uppercase select-none"
            onclick={() => toggleSection('recent')}
          >
            <span>RECENT</span>
            {#if collapsedSections['recent']}
              <ChevronRightIcon class="size-3" />
            {:else}
              <ChevronDownIcon class="size-3" />
            {/if}
          </button>
          {#if !collapsedSections['recent']}
            <ul class="flex list-none flex-col gap-0.5 p-0">
              {#each recentSessions as session (session.session_id)}
                {@render sessionRow(session)}
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    {/if}
  </div>

  <div class="border-t p-2 flex flex-col gap-2">
    <Button
      variant="outline"
      size="sm"
      class="w-full justify-center gap-1.5 text-xs"
      onclick={() => {
        handleLinkClick();
        window.location.href = '/';
      }}
    >
      <PlusIcon class="size-3.5" />
      新規セッションを開始
    </Button>

    <div class="flex items-center justify-between text-xs pt-1">
      <SyncStatusBadge />
      <div class="flex items-center gap-1">
        <ConnectionMenu />
        <KillSwitchButton />
      </div>
    </div>
  </div>
</aside>

{#snippet sessionRow(session: SessionSummary)}
  {@const isActive = activeSessionId === session.session_id}
  {@const node = getNode(session.node_id)}
  {@const nodeAvail = getNodeAvailability(node)}
  {@const isPinned = pinned.isPinned(session.session_id)}
  <li>
    <a
      href={`/sessions/${session.session_id}`}
      onclick={handleLinkClick}
      class={cn(
        'group flex items-center justify-between rounded-md px-2 py-1.5 text-xs transition-colors',
        isActive
          ? 'bg-accent text-accent-foreground font-medium'
          : 'text-foreground/80 hover:bg-accent/40 hover:text-foreground'
      )}
    >
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <span
          class={cn('size-1.5 shrink-0 rounded-full', nodeAvail.dotColorClass)}
          title={`ノード: ${node?.name ?? session.node_id} (${nodeAvail.statusText})`}
        ></span>
        <span class="truncate">{session.title}</span>
      </div>

      <div class="ml-2 flex shrink-0 items-center gap-1.5 text-muted-foreground text-[10px]">
        <button
          type="button"
          class={cn(
            'hover:text-foreground p-0.5 transition-opacity',
            isPinned ? 'opacity-100 text-amber-500' : 'opacity-0 group-hover:opacity-100'
          )}
          title={isPinned ? 'ピン留め解除' : 'ピン留め'}
          onclick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            pinned.toggle(session.session_id);
          }}
        >
          <PinIcon class="size-3" fill={isPinned ? 'currentColor' : 'none'} />
        </button>

        <span>{formatRelativeTimeCompact(session.updated_at)}</span>
      </div>
    </a>
  </li>
{/snippet}
