<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { m } from "#lib/paraglide/messages.js";

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

  let value = $state("");
  let busy = $state(false);

  // Reset the input whenever the dialog opens (or the target draft changes).
  $effect(() => {
    if (open) value = title;
  });

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const next = value.trim();
    if (busy || next === "") return;
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
    <form class="flex flex-col gap-4" onsubmit={(event) => void submit(event)}>
      <div class="grid gap-2">
        <Label for="draft-title">{m.drafts_name()}</Label>
        <Input id="draft-title" bind:value placeholder={m.drafts_untitled()} />
      </div>
      <Dialog.Footer>
        <Button type="button" variant="ghost" onclick={() => (open = false)}>
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={busy || value.trim() === ""}>
          {m.common_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
