<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import DraftRenameForm from "./DraftRenameForm.svelte";

  let {
    open = $bindable(false),
    title,
    onsave,
  }: {
    /** Whether the dialog is visible. */
    open: boolean;
    /** Current draft title (empty = derive from the body). */
    title: string;
    /** Called with the trimmed title; the dialog closes when it resolves. */
    onsave: (title: string) => void | Promise<void>;
  } = $props();

  let busy = $state(false);

  async function submit(next: string): Promise<void> {
    busy = true;
    try {
      await onsave(next);
      open = false;
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.drafts_rename_title()}</Dialog.Title>
      <Dialog.Description>{m.drafts_rename_description()}</Dialog.Description>
    </Dialog.Header>
    <DraftRenameForm {title} {busy} onsubmit={submit} oncancel={() => (open = false)} />
  </Dialog.Content>
</Dialog.Root>
