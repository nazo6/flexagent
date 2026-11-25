<script lang="ts">
  /**
   * Non-text content blocks received with a message (images, audio, resources).
   *
   * Inline images render directly; everything else is listed by name so the
   * payload is still visible when the node does not ship the bytes inline.
   */
  import type { AttachmentMeta } from "#lib/generated/bindings.ts";
  import { cn } from "#lib/utils.ts";
  import PaperclipIcon from "@lucide/svelte/icons/paperclip";

  let {
    attachments,
    inBubble = false,
  }: {
    attachments: AttachmentMeta[];
    /** User message bubble styling (translucent chip on the bubble color). */
    inBubble?: boolean;
  } = $props();
</script>

{#if attachments.length > 0}
  <div class="mb-1 flex flex-wrap gap-1.5">
    {#each attachments as attachment, index (`${index}:${attachment.file_name}:${attachment.local_path ?? ""}`)}
      {#if attachment.data_b64 && attachment.mime_type.startsWith("image/")}
        <img
          src={`data:${attachment.mime_type};base64,${attachment.data_b64}`}
          alt={attachment.file_name}
          class="max-h-40 rounded-md object-contain"
        />
      {:else}
        <span
          class={cn(
            "inline-flex max-w-full items-center gap-1 rounded px-1.5 py-0.5 text-[11px]",
            inBubble ? "bg-background/60" : "bg-muted/60 text-muted-foreground",
          )}
        >
          <PaperclipIcon class="size-2.5 shrink-0" />
          <span class="truncate">{attachment.file_name}</span>
        </span>
      {/if}
    {/each}
  </div>
{/if}
