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
  import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';
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
  let collapsedProjects = $state<Record<string, boolean>>({});
  let showAttentionSection = $state(true);

  const currentPath = $derived(page.url.pathname);
  const activeSessionId = $derived(
    currentPath.startsWith('/sessions/') ? currentPath.replace('/sessions/', '') : null
  );

  function toggleProject(projectId: string) {
    collapsedProjects[projectId] = !collapsedProjects[projectId];
  }

  function isProjectCollapsed(projectId: string): boolean {
    if (searchQuery.trim() !== '') return false;
    return collapsedProjects[projectId] ?? false;
  }

  function matchesSearch(session: SessionSummary, query: string): boolean {
    if (!query) return true;
    const q = query.toLowerCase();
    return (
      session.title.toLowerCase().includes(q) ||
      session.project_id.toLowerCase().includes(q) ||
      session.local_path.toLowerCase().includes(q) ||
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

  // 全体横断の要対応セッション
  const needsAttentionSessions = $derived(
    filteredSessions.filter(
      (s) => pendingSessionIds.has(s.session_id) || s.status === 'error'
    )
  );

  interface ProjectSessionGroup {
    projectId: string;
    projectName: string;
    sessions: SessionSummary[];
    pendingCount: number;
    runningCount: number;
  }

  // プロジェクト別のセッショングループ
  const projectGroups = $derived.by(() => {
    const groups: ProjectSessionGroup[] = [];
    const projectMap = new Map<string, string>();

    for (const p of sync.projects) {
      projectMap.set(p.project_id, p.name);
    }

    const allProjectIds = new Set<string>();
    for (const s of filteredSessions) {
      allProjectIds.add(s.project_id);
    }
    for (const p of sync.projects) {
      allProjectIds.add(p.project_id);
    }

    for (const pId of allProjectIds) {
      const projSessions = filteredSessions
        .filter((s) => s.project_id === pId)
        .toSorted((a, b) => {
          const aPinned = pinned.isPinned(a.session_id) ? 1 : 0;
          const bPinned = pinned.isPinned(b.session_id) ? 1 : 0;
          if (aPinned !== bPinned) return bPinned - aPinned;
          return b.updated_at - a.updated_at;
        });

      if (searchQuery.trim() !== '' && projSessions.length === 0) {
        continue;
      }

      const pendingCount = projSessions.filter((s) => pendingSessionIds.has(s.session_id)).length;
      const runningCount = projSessions.filter((s) => s.status === 'running').length;

      groups.push({
        projectId: pId,
        projectName: projectMap.get(pId) ?? pId,
        sessions: projSessions,
        pendingCount,
        runningCount
      });
    }

    // 直近更新セッションがあるプロジェクトを上位にソート
    return groups.toSorted((a, b) => {
      const aLatest = a.sessions[0]?.updated_at ?? 0;
      const bLatest = b.sessions[0]?.updated_at ?? 0;
      return bLatest - aLatest;
    });
  });

  function getNode(nodeId: string) {
    return sync.nodes.find((n) => n.node_id === nodeId);
  }

  function handleLinkClick() {
    onNavigate?.();
  }
</script>

<aside class="bg-card flex h-full w-full flex-col border-r">
  <!-- サイドバーヘッダー -->
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

  <!-- クイックナビゲーション -->
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
      <span>プロジェクト管理</span>
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
      <span>全文検索</span>
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

  <!-- 検索バー -->
  <div class="border-b px-2 py-2">
    <div class="relative flex items-center">
      <SearchIcon class="text-muted-foreground absolute left-2 size-3.5 pointer-events-none" />
      <input
        type="text"
        bind:value={searchQuery}
        placeholder="セッション・プロジェクトを検索…"
        class="bg-muted/40 placeholder:text-muted-foreground focus-visible:ring-ring/40 w-full rounded-md border py-1 pr-2 pl-7 text-xs outline-none focus-visible:ring-1"
      />
    </div>
  </div>

  <!-- セッション一覧リスト (プロジェクト別) -->
  <div class="scrollbar-thin flex-1 overflow-y-auto p-2">
    <!-- 要対応セクション (承認待ち・エラーがある場合のみ最上部に表示) -->
    {#if needsAttentionSessions.length > 0 && searchQuery.trim() === ''}
      <div class="mb-3 rounded-lg border border-destructive/30 bg-destructive/5 p-1.5">
        <button
          type="button"
          class="text-destructive flex w-full items-center justify-between px-1 py-0.5 text-[11px] font-semibold tracking-wider uppercase select-none"
          onclick={() => (showAttentionSection = !showAttentionSection)}
        >
          <span class="flex items-center gap-1.5">
            <AlertCircleIcon class="size-3" />
            <span>NEEDS ATTENTION</span>
            <span class="rounded-full bg-destructive/15 px-1.5 py-0.2 text-[10px] text-destructive">
              {needsAttentionSessions.length}
            </span>
          </span>
          {#if showAttentionSection}
            <ChevronDownIcon class="size-3" />
          {:else}
            <ChevronRightIcon class="size-3" />
          {/if}
        </button>

        {#if showAttentionSection}
          <ul class="flex list-none flex-col gap-0.5 p-0 mt-1">
            {#each needsAttentionSessions as session (session.session_id)}
              {@render sessionRow(session, true)}
            {/each}
          </ul>
        {/if}
      </div>
    {/if}

    <!-- プロジェクト別一覧 -->
    {#if projectGroups.length === 0}
      <div class="text-muted-foreground p-3 text-center text-xs">
        {searchQuery ? '該当するプロジェクトまたはセッションがありません' : 'プロジェクトがまだありません'}
      </div>
    {:else}
      <div class="flex flex-col gap-2.5">
        {#each projectGroups as group (group.projectId)}
          {@const isCollapsed = isProjectCollapsed(group.projectId)}
          <div class="flex flex-col">
            <!-- プロジェクト見出しヘッダー -->
            <div class="group/proj flex items-center justify-between rounded-md px-1.5 py-1 text-xs hover:bg-muted/40 transition-colors">
              <button
                type="button"
                class="flex min-w-0 flex-1 items-center gap-1.5 text-left font-semibold text-foreground/90 select-none"
                onclick={() => toggleProject(group.projectId)}
              >
                <span class="text-muted-foreground shrink-0">
                  {#if isCollapsed}
                    <ChevronRightIcon class="size-3.5" />
                  {:else}
                    <ChevronDownIcon class="size-3.5" />
                  {/if}
                </span>
                <FolderGit2Icon class="size-3.5 text-muted-foreground shrink-0" />
                <span class="truncate">{group.projectName}</span>
                {#if group.pendingCount > 0}
                  <Badge variant="destructive" class="px-1 py-0 text-[9px] leading-tight shrink-0">
                    {group.pendingCount}
                  </Badge>
                {/if}
                {#if group.runningCount > 0}
                  <span class="size-1.5 rounded-full bg-emerald-500 shrink-0" title="実行中セッションあり"></span>
                {/if}
              </button>

              <div class="flex items-center gap-1 shrink-0 ml-1">
                <span class="text-muted-foreground text-[10px]">
                  {group.sessions.length}
                </span>
                <a
                  href={`/?project=${encodeURIComponent(group.projectId)}`}
                  onclick={handleLinkClick}
                  class="opacity-0 group-hover/proj:opacity-100 hover:bg-accent text-muted-foreground hover:text-foreground rounded p-0.5 transition-opacity"
                  title={`${group.projectName} で新規セッションを開始`}
                >
                  <PlusIcon class="size-3" />
                </a>
              </div>
            </div>

            <!-- プロジェクト内セッションリスト -->
            {#if !isCollapsed}
              <ul class="ml-2.5 border-l border-border/60 pl-2 flex list-none flex-col gap-0.5 p-0 mt-0.5">
                {#each group.sessions as session (session.session_id)}
                  {@render sessionRow(session, false)}
                {/each}
                {#if group.sessions.length === 0}
                  <li class="py-1 px-1.5 text-[11px] text-muted-foreground">
                    <a
                      href={`/?project=${encodeURIComponent(group.projectId)}`}
                      onclick={handleLinkClick}
                      class="hover:underline flex items-center gap-1 text-primary/80"
                    >
                      <PlusIcon class="size-3" />
                      <span>セッションを開始</span>
                    </a>
                  </li>
                {/if}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- 下部フッター: 新規ボタン & ステータス -->
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

{#snippet sessionRow(session: SessionSummary, showProjectBadge: boolean)}
  {@const isActive = activeSessionId === session.session_id}
  {@const node = getNode(session.node_id)}
  {@const nodeAvail = getNodeAvailability(node)}
  {@const isPinned = pinned.isPinned(session.session_id)}
  <li>
    <a
      href={`/sessions/${session.session_id}`}
      onclick={handleLinkClick}
      title={`${session.title}\nワーキングディレクトリ: ${session.local_path}\nノード: ${node?.name ?? session.node_id}`}
      class={cn(
        'group flex items-center justify-between rounded-md px-2 py-1.5 text-xs transition-colors',
        isActive
          ? 'bg-accent text-accent-foreground font-medium'
          : 'text-foreground/80 hover:bg-accent/40 hover:text-foreground'
      )}
    >
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <!-- ノードステータスドット -->
        <span
          class={cn('size-1.5 shrink-0 rounded-full', nodeAvail.dotColorClass)}
          title={`ノード: ${node?.name ?? session.node_id} (${nodeAvail.statusText})`}
        ></span>

        <div class="flex min-w-0 flex-1 items-center gap-1">
          {#if showProjectBadge}
            <span class="text-muted-foreground text-[10px] shrink-0 font-medium truncate max-w-16">
              [{session.project_id}]
            </span>
          {/if}
          {#if session.git_branch}
            <span class="text-muted-foreground text-[10px] shrink-0 font-mono">
              ({session.git_branch})
            </span>
          {/if}
          <!-- タイトル -->
          <span class="truncate">{session.title}</span>
        </div>
      </div>

      <div class="ml-2 flex shrink-0 items-center gap-1.5 text-muted-foreground text-[10px]">
        <!-- ピン留めボタン -->
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
