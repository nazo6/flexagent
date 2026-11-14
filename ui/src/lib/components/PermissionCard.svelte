<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import type { PermissionOption } from "#lib/generated/bindings.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import CheckIcon from "@lucide/svelte/icons/check";
  import ShieldCheckIcon from "@lucide/svelte/icons/shield-check";
  import XIcon from "@lucide/svelte/icons/x";

  let {
    sessionId,
    requestId,
    toolName,
    summary,
    options,
    details = null,
    resolved = null,
    createdAt = null,
    subagentTitle = null,
  }: {
    sessionId: string;
    requestId: string;
    toolName: string;
    summary: string;
    options: PermissionOption[];
    details?: unknown;
    resolved?: { selectedOptionId: string; resolvedBy: string } | null;
    createdAt?: number | null;
    subagentTitle?: string | null;
  } = $props();

  let busyOptionId = $state<string | null>(null);

  const detailsJson = $derived(
    details === null || details === undefined ? null : JSON.stringify(details, null, 2),
  );

  function isAllowOption(kind: string): boolean {
    return kind.startsWith("allow");
  }

  async function respond(option: PermissionOption) {
    if (busyOptionId !== null) return;
    busyOptionId = option.option_id;
    try {
      const result = await sync.respondPermission(sessionId, requestId, option.option_id);
      if (result.already_resolved) {
        toast.info(m.error_already_resolved());
      } else {
        toast.success(m.chat_permission_sent());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyOptionId = null;
    }
  }
</script>

<div class="bg-card border-border flex flex-col gap-3 rounded-lg border p-3">
  <div class="flex min-w-0 flex-col gap-1">
    <div class="flex items-center gap-2">
      <ShieldCheckIcon class="text-muted-foreground size-4 shrink-0" />
      <span class="text-warn text-[11px] font-semibold tracking-wider uppercase">
        {m.chat_permission_required()}
      </span>
      <span class="desk-mono text-[11px] font-medium">{toolName}</span>
      {#if subagentTitle}
        <Badge variant="outline">{m.chat_subagent_title()}: {subagentTitle}</Badge>
      {/if}
      {#if resolved}
        <Badge variant="secondary">
          {resolved.selectedOptionId} ({resolved.resolvedBy})
        </Badge>
      {:else if createdAt !== null}
        <span class="text-muted-foreground text-[11px]">{formatRelativeTime(createdAt)}</span>
      {/if}
    </div>
    <p class="text-sm break-words">{summary}</p>
  </div>

  {#if detailsJson !== null}
    <details class="group">
      <summary
        class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px] select-none"
      >
        {m.chat_permission_details()}
      </summary>
      <pre
        class="bg-muted/50 border-border mt-2 max-h-56 overflow-auto rounded-md border p-2 font-mono text-[13px] whitespace-pre-wrap">{detailsJson}</pre>
    </details>
  {/if}

  {#if resolved === null}
    <div class="flex flex-wrap gap-2">
      {#each options as option (option.option_id)}
        <Button
          size="sm"
          variant={isAllowOption(option.kind) ? "default" : "outline"}
          disabled={busyOptionId !== null}
          onclick={() => void respond(option)}
        >
          {#if isAllowOption(option.kind)}
            <CheckIcon />
          {:else}
            <XIcon />
          {/if}
          {option.name || option.option_id}
        </Button>
      {/each}
    </div>
  {/if}
</div>
