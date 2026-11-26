<script lang="ts">
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import type { AttachmentMeta } from "#lib/generated/bindings.ts";
  import FileIcon from "@lucide/svelte/icons/file";
  import XIcon from "@lucide/svelte/icons/x";

  let {
    attachments,
    onremove,
    class: className = "",
  }: {
    /** Attachments currently queued in the composer. */
    attachments: readonly AttachmentMeta[];
    /** Remove the attachment at `index`. */
    onremove: (index: number) => void;
    class?: string;
  } = $props();
</script>

{#if attachments.length > 0}
  <div class={cn("flex flex-wrap gap-2", className)}>
    {#each attachments as attachment, index (index)}
      <div
        class="bg-chip border-border flex items-center gap-1.5 rounded-md border px-2 py-1 text-[13px]"
      >
        {#if attachment.data_b64}
          <img
            src={`data:${attachment.mime_type};base64,${attachment.data_b64}`}
            alt={attachment.file_name}
            class="size-8 rounded object-cover"
          />
        {:else}
          <FileIcon class="text-muted-foreground size-3.5" />
        {/if}
        <span class="max-w-40 truncate">{attachment.file_name}</span>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground"
          title={m.chat_attachment_remove()}
          onclick={() => onremove(index)}
        >
          <XIcon class="size-3.5" />
        </button>
      </div>
    {/each}
  </div>
{/if}
