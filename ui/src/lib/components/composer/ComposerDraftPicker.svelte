<script lang="ts">
  import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
  import { formatRelativeTimeCompact } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { composerDrafts, draftTitle } from "#lib/stores/composer-drafts.ts";
  import DraftRenameDialog from "./DraftRenameDialog.svelte";
  import FilesIcon from "@lucide/svelte/icons/files";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let {
    scope,
    sessionId,
  }: {
    /** Draft scope (`session:<id>` or `new`). */
    scope: string;
    /** Target session (`null` on the new-session screen). */
    sessionId: string | null;
  } = $props();

  const drafts = $derived(composerDrafts.list(scope));
  const activeDraft = $derived(composerDrafts.activeDraft(scope));
  const activeId = $derived(activeDraft?.draft_id ?? null);
  let renameOpen = $state(false);

  function removeActive(): void {
    if (activeId === null || activeDraft === null) return;
    if (!confirm(m.drafts_delete_confirm({ title: draftTitle(activeDraft) }))) return;
    void composerDrafts.remove(scope, activeId, sessionId, sync.connection.client);
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class="text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/40 inline-flex h-7 shrink-0 items-center justify-center gap-1 rounded-full px-1.5 outline-none motion-press focus-visible:ring-2"
    title={m.drafts_title()}
    aria-label={m.drafts_title()}
  >
    <FilesIcon class="size-3.5" />
    {#if drafts.length > 0}
      <span class="text-[11px] tabular-nums">{drafts.length}</span>
    {/if}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" class="w-72">
    {#each drafts as draft (draft.draft_id)}
      <DropdownMenu.Item
        onSelect={() => composerDrafts.select(scope, draft.draft_id)}
        class={draft.draft_id === activeId ? "bg-muted" : ""}
      >
        <span class="flex min-w-0 flex-1 items-center gap-2">
          <span class="text-signal w-2 shrink-0 text-[10px]">
            {draft.draft_id === activeId ? "●" : ""}
          </span>
          <span class="min-w-0 flex-1 truncate">{draftTitle(draft)}</span>
          <span class="text-muted-foreground shrink-0 text-[11px]">
            {formatRelativeTimeCompact(draft.updated_at)}
          </span>
        </span>
      </DropdownMenu.Item>
    {/each}
    <DropdownMenu.Separator />
    <DropdownMenu.Item
      onSelect={() => void composerDrafts.create(scope, sessionId, sync.connection.client)}
    >
      <PlusIcon />
      {m.drafts_new()}
    </DropdownMenu.Item>
    {#if activeDraft !== null}
      <DropdownMenu.Item onSelect={() => (renameOpen = true)}>
        <PencilIcon />
        {m.drafts_rename()}
      </DropdownMenu.Item>
    {/if}
    {#if drafts.length > 1 && activeId !== null}
      <DropdownMenu.Item onSelect={removeActive}>
        <Trash2Icon />
        {m.drafts_delete()}
      </DropdownMenu.Item>
    {/if}
  </DropdownMenu.Content>
</DropdownMenu.Root>

<DraftRenameDialog
  bind:open={renameOpen}
  title={activeDraft?.title ?? ""}
  onsave={(title) => {
    if (activeId !== null) composerDrafts.rename(scope, activeId, title);
  }}
/>
