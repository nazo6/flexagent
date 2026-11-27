<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import DraftRenameDialog from "#lib/components/composer/DraftRenameDialog.svelte";
  import { formatRelativeTimeCompact } from "#lib/format.ts";
  import type { Draft } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { draftTitle, loadDeviceId } from "#lib/stores/composer-drafts.ts";
  import { problems } from "#lib/stores/problems.svelte.ts";
  import ArrowUpRightIcon from "@lucide/svelte/icons/arrow-up-right";
  import FileTextIcon from "@lucide/svelte/icons/file-text";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let drafts = $state<Draft[]>([]);
  let loading = $state(true);
  let refreshing = false;
  let loadError = $state<string | null>(null);
  let renameOpen = $state(false);
  let renameTarget = $state<Draft | null>(null);

  interface DraftGroup {
    projectId: string;
    name: string;
    items: Draft[];
  }

  const groups = $derived.by((): DraftGroup[] => {
    const byProject = new Map<string, Draft[]>();
    for (const draft of drafts) {
      const key = draft.project_id ?? "";
      byProject.set(key, [...(byProject.get(key) ?? []), draft]);
    }
    return [...byProject.entries()]
      .map(([projectId, items]) => ({
        projectId,
        name:
          projectId === ""
            ? m.drafts_no_project()
            : (sync.projects.find((project) => project.project_id === projectId)?.name ??
              projectId),
        items: items.toSorted((a, b) => b.updated_at - a.updated_at),
      }))
      .toSorted((a, b) => (b.items[0]?.updated_at ?? 0) - (a.items[0]?.updated_at ?? 0));
  });

  async function refresh(): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    loading = true;
    loadError = null;
    try {
      drafts = await sync.connection.client.drafts({ sessionless: true, limit: 200 });
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
      problems.report("load drafts", error);
    } finally {
      refreshing = false;
      loading = false;
    }
  }

  function openDraft(draft: Draft): void {
    const params = new URLSearchParams({ draft: draft.draft_id });
    if (draft.project_id !== null) params.set("project", draft.project_id);
    if (draft.node_id !== null) params.set("node", draft.node_id);
    void goto(`/?${params.toString()}`);
  }

  function startRename(draft: Draft): void {
    renameTarget = draft;
    renameOpen = true;
  }

  async function saveRename(title: string): Promise<void> {
    const draft = renameTarget;
    if (draft === null) return;
    try {
      const updated = await sync.connection.client.updateDraft(draft.draft_id, {
        expected_revision: draft.revision,
        title,
        body: draft.body,
        session_id: draft.session_id,
        project_id: draft.project_id,
        node_id: draft.node_id,
        attachments: draft.attachments ?? [],
        origin_device_id: loadDeviceId(),
      });
      drafts = drafts.map((item) => (item.draft_id === updated.draft_id ? updated : item));
      renameTarget = null;
    } catch (error) {
      problems.report("rename draft", error);
      // Another device may have changed it; show the current server state.
      await refresh();
      throw error;
    }
  }

  async function deleteDraft(draft: Draft): Promise<void> {
    if (!confirm(m.drafts_delete_confirm({ title: draftTitle(draft) }))) return;
    try {
      await sync.connection.client.deleteDraft(draft.draft_id);
      drafts = drafts.filter((item) => item.draft_id !== draft.draft_id);
    } catch (error) {
      problems.report("delete draft", error);
    }
  }

  function preview(body: string): string {
    const text = body.trim();
    return text === "" ? m.drafts_empty_body() : text;
  }

  onMount(() => {
    void refresh();
  });

  // Reconnect (or the initial WS handshake) may complete after the page mounts.
  $effect(() => {
    if (sync.wsConnected) void refresh();
  });
</script>

<svelte:head>
  <title>{m.drafts_page_title()}</title>
</svelte:head>

<div class="desk-page">
  <div class="flex flex-wrap items-end justify-between gap-3">
    <div class="flex flex-col gap-1">
      <h1 class="text-xl font-semibold tracking-tight">{m.drafts_title()}</h1>
      <p class="text-muted-foreground text-[13px]">{m.drafts_subtitle()}</p>
    </div>
    <div class="flex items-center gap-1.5">
      <Button variant="outline" size="sm" onclick={() => void refresh()}>
        <RefreshCwIcon />
        {m.common_refresh()}
      </Button>
      <Button size="sm" onclick={() => void goto("/")}>
        <PlusIcon />
        {m.drafts_new()}
      </Button>
    </div>
  </div>

  {#if loading}
    <p class="text-muted-foreground text-[13px]">{m.common_loading()}</p>
  {:else if loadError !== null}
    <div
      class="border-danger/25 bg-danger-soft/40 text-danger rounded-md border px-3 py-2 text-[13px]"
    >
      {loadError}
    </div>
  {:else if groups.length === 0}
    <div
      class="border-border flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-12 text-center"
    >
      <FileTextIcon class="text-muted-foreground size-8" />
      <p class="font-medium">{m.drafts_empty_title()}</p>
      <p class="text-muted-foreground text-[13px]">{m.drafts_empty_description()}</p>
      <Button size="sm" class="mt-2" onclick={() => void goto("/")}>
        <PlusIcon />
        {m.drafts_new()}
      </Button>
    </div>
  {:else}
    {#each groups as group (group.projectId)}
      <section class="flex flex-col gap-2">
        <h2 class="text-muted-foreground text-[11px] font-medium tracking-wide uppercase">
          {group.name}
        </h2>
        <ul class="border-border divide-border divide-y overflow-hidden rounded-lg border">
          {#each group.items as draft (draft.draft_id)}
            <li class="hover:bg-muted/40 flex items-start gap-2 px-3 py-2.5 transition-colors">
              <button
                type="button"
                class="min-w-0 flex-1 text-left"
                onclick={() => openDraft(draft)}
              >
                <div class="flex items-center gap-2">
                  <span class="truncate text-sm font-medium">{draftTitle(draft)}</span>
                  <span class="text-muted-foreground shrink-0 text-[11px]">
                    {formatRelativeTimeCompact(draft.updated_at)}
                  </span>
                </div>
                <p
                  class="text-muted-foreground mt-0.5 line-clamp-2 text-[13px] whitespace-pre-line"
                >
                  {preview(draft.body)}
                </p>
              </button>
              <div class="flex shrink-0 items-center gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  title={m.drafts_open()}
                  aria-label={m.drafts_open()}
                  onclick={() => openDraft(draft)}
                >
                  <ArrowUpRightIcon />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  title={m.drafts_rename()}
                  aria-label={m.drafts_rename()}
                  onclick={() => startRename(draft)}
                >
                  <PencilIcon />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  title={m.drafts_delete()}
                  aria-label={m.drafts_delete()}
                  onclick={() => void deleteDraft(draft)}
                >
                  <Trash2Icon />
                </Button>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</div>

<DraftRenameDialog bind:open={renameOpen} title={renameTarget?.title ?? ""} onsave={saveRename} />
