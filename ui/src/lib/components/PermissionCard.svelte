<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import type { PermissionOption } from "#lib/generated/bindings.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { permissionOptionLabel } from "#lib/session-status.ts";
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

  const resolvedOption = $derived(
    resolved === null
      ? null
      : (options.find((option) => option.option_id === resolved.selectedOptionId) ?? null),
  );
  const resolvedIsReject = $derived(resolvedOption?.kind.startsWith("reject") ?? false);
  const autoResolved = $derived(resolved?.resolvedBy === "auto");
  const heading = $derived(
    resolved === null
      ? m.chat_permission_required()
      : autoResolved
        ? m.chat_permission_auto_approved()
        : resolvedIsReject
          ? m.chat_permission_denied()
          : m.chat_permission_granted(),
  );
  const headingTone = $derived(
    resolved === null
      ? "text-warn"
      : resolvedIsReject
        ? "text-danger"
        : autoResolved
          ? "text-muted-foreground"
          : "text-signal",
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
      <ShieldCheckIcon class="size-4 shrink-0 {headingTone}" />
      <span class="text-[11px] font-semibold tracking-wider uppercase {headingTone}">
        {heading}
      </span>
      <span class="desk-mono text-[11px] font-medium">{toolName}</span>
      {#if subagentTitle}
        <Badge variant="outline">{m.chat_subagent_title()}: {subagentTitle}</Badge>
      {/if}
      {#if resolvedOption}
        <Badge variant="secondary">
          {permissionOptionLabel(resolvedOption.name, resolvedOption.kind)}
        </Badge>
      {:else if resolved}
        <Badge variant="secondary">{resolved.selectedOptionId}</Badge>
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
