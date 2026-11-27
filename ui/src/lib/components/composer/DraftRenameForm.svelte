<script lang="ts">
  import { untrack } from "svelte";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { m } from "#lib/paraglide/messages.js";

  let {
    title,
    busy,
    onsubmit,
    oncancel,
  }: {
    /** Current draft title (empty = derive from the body). */
    title: string;
    busy: boolean;
    /** Called with the trimmed title; the dialog closes when it resolves. */
    onsubmit: (title: string) => void | Promise<void>;
    oncancel: () => void;
  } = $props();

  // The dialog content is mounted per open, so seeding at construction is a
  // fresh start every time without an effect copying `title` on open.
  let value = $state(untrack(() => title));

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    const next = value.trim();
    if (busy || next === "") return;
    void onsubmit(next);
  }
</script>

<form class="flex flex-col gap-4" onsubmit={submit}>
  <div class="grid gap-2">
    <Label for="draft-title">{m.drafts_name()}</Label>
    <Input id="draft-title" bind:value placeholder={m.drafts_untitled()} />
  </div>
  <Dialog.Footer>
    <Button type="button" variant="ghost" onclick={oncancel}>
      {m.common_cancel()}
    </Button>
    <Button type="submit" disabled={busy || value.trim() === ""}>
      {m.common_save()}
    </Button>
  </Dialog.Footer>
</form>
