import { createMemo, createSignal, For, Show } from "solid-js";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import type { PermissionOption } from "#lib/generated/bindings.ts";
import { formatRelativeTime } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import { permissionOptionLabel } from "#lib/session-status.ts";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconCheck from "~icons/lucide/check";
import IconShieldCheck from "~icons/lucide/shield-check";
import IconX from "~icons/lucide/x";

export interface PermissionCardProps {
  sessionId: string;
  requestId: string;
  toolName: string;
  summary: string;
  options: PermissionOption[];
  details?: unknown;
  resolved?: { selectedOptionId: string; resolvedBy: string } | null;
  createdAt?: number | null;
  subagentTitle?: string | null;
}

/** Inline approval card for one `permission/request` (pending or resolved). */
export default function PermissionCard(props: PermissionCardProps) {
  const [busyOptionId, setBusyOptionId] = createSignal<string | null>(null);

  const detailsJson = () =>
    props.details === null || props.details === undefined
      ? null
      : JSON.stringify(props.details, null, 2);

  const resolvedOption = createMemo(() => {
    const resolved = props.resolved ?? null;
    return resolved === null
      ? null
      : (props.options.find((option) => option.option_id === resolved.selectedOptionId) ?? null);
  });
  const resolvedIsReject = () => resolvedOption()?.kind.startsWith("reject") ?? false;
  const autoResolved = () => props.resolved?.resolvedBy === "auto";
  const heading = () =>
    (props.resolved ?? null) === null
      ? m.chat_permission_required()
      : autoResolved()
        ? m.chat_permission_auto_approved()
        : resolvedIsReject()
          ? m.chat_permission_denied()
          : m.chat_permission_granted();
  const headingTone = () =>
    (props.resolved ?? null) === null
      ? "text-warn"
      : resolvedIsReject()
        ? "text-danger"
        : autoResolved()
          ? "text-muted-foreground"
          : "text-signal";

  function isAllowOption(kind: string): boolean {
    return kind.startsWith("allow");
  }

  async function respond(option: PermissionOption): Promise<void> {
    if (busyOptionId() !== null) return;
    setBusyOptionId(option.option_id);
    try {
      const result = await sync.respondPermission(props.sessionId, props.requestId, option.option_id);
      if (result.already_resolved) {
        toast.info(m.error_already_resolved());
      } else {
        toast.success(m.chat_permission_sent());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyOptionId(null);
    }
  }

  return (
    <div class="bg-card border-border flex flex-col gap-3 rounded-lg border p-3">
      <div class="flex min-w-0 flex-col gap-1">
        <div class="flex items-center gap-2">
          <IconShieldCheck class={cn("size-4 shrink-0", headingTone())} />
          <span class={cn("text-[11px] font-semibold tracking-wider uppercase", headingTone())}>
            {heading()}
          </span>
          <span class="desk-mono text-[11px] font-medium">{props.toolName}</span>
          <Show when={props.subagentTitle}>
            {(title) => (
              <Badge variant="outline">
                {m.chat_subagent_title()}: {title()}
              </Badge>
            )}
          </Show>
          <Show
            when={resolvedOption()}
            fallback={
              <Show
                when={props.resolved}
                fallback={
                  <Show when={props.createdAt !== null && props.createdAt !== undefined}>
                    <span class="text-muted-foreground text-[11px]">
                      {formatRelativeTime(props.createdAt ?? null)}
                    </span>
                  </Show>
                }
              >
                {(resolved) => <Badge variant="secondary">{resolved().selectedOptionId}</Badge>}
              </Show>
            }
          >
            {(option) => (
              <Badge variant="secondary">
                {permissionOptionLabel(option().name, option().kind)}
              </Badge>
            )}
          </Show>
        </div>
        <p class="text-sm break-words">{props.summary}</p>
      </div>

      <Show when={detailsJson()}>
        {(json) => (
          <details class="group">
            <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px] select-none">
              {m.chat_permission_details()}
            </summary>
            <pre class="bg-muted/50 border-border mt-2 max-h-56 overflow-auto rounded-md border p-2 font-mono text-[13px] whitespace-pre-wrap">
              {json()}
            </pre>
          </details>
        )}
      </Show>

      <Show when={(props.resolved ?? null) === null}>
        <div class="flex flex-wrap gap-2">
          <For each={props.options}>
            {(option) => (
              <Button
                size="sm"
                variant={isAllowOption(option.kind) ? "default" : "outline"}
                disabled={busyOptionId() !== null}
                onClick={() => void respond(option)}
              >
                <Show when={isAllowOption(option.kind)} fallback={<IconX />}>
                  <IconCheck />
                </Show>
                {option.name || option.option_id}
              </Button>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
