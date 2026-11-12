<script lang="ts">
  import * as Popover from "#lib/components/ui/popover/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import ContextMeter from "#lib/components/session/ContextMeter.svelte";
  import NodeStatusBadge from "#lib/components/NodeStatusBadge.svelte";
  import type {
    NodeSummary,
    SessionStatus,
    SessionSummary,
    SessionUsage,
  } from "#lib/generated/bindings.ts";
  import { formatEpochMs } from "#lib/format.ts";
  import { sessionStatusLabel } from "#lib/session-status.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import CopyIcon from "@lucide/svelte/icons/copy";

  interface Props {
    session: SessionSummary | null;
    node: NodeSummary | null;
    status: SessionStatus | null;
    usage: SessionUsage | null;
    projectName: string | null;
    class?: string;
  }

  let { session, node, status, usage, projectName, class: className = "" }: Props = $props();

  const permissionLabel = $derived(
    session?.permission_policy === "auto_allow_all"
      ? m.session_details_permission_auto()
      : m.session_details_permission_ask(),
  );

  async function copy(value: string, message: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(message);
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }
</script>

{#if session}
  <Popover.Root>
    <Popover.Trigger
      class={cn(
        "text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/40 inline-flex min-w-0 items-center gap-1.5 rounded-md px-1.5 py-1 text-[11px] outline-none transition-colors focus-visible:ring-2",
        className,
      )}
      title={m.session_details_title()}
    >
      <NodeStatusBadge {node} showNodeName showLabel={false} />
      {#if session.git_branch}
        <span class="opacity-40">·</span>
        <span class="min-w-0 truncate font-mono">{session.git_branch}</span>
      {/if}
      <ChevronDownIcon class="size-3 shrink-0" />
    </Popover.Trigger>

    <Popover.Content align="start" class="w-[min(23rem,calc(100vw-2rem))] p-0">
      <div class="border-b border-border px-3 py-2">
        <h2 class="text-[13px] font-semibold">{m.session_details_title()}</h2>
      </div>

      <dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2.5 p-3 text-[12px]">
        <dt class="text-muted-foreground">{m.session_details_project()}</dt>
        <dd class="min-w-0 break-words">
          {projectName ?? session.project_id}
          {#if projectName}
            <span class="text-muted-foreground block truncate font-mono text-[11px]">
              {session.project_id}
            </span>
          {/if}
        </dd>

        <dt class="text-muted-foreground">{m.session_details_node()}</dt>
        <dd class="flex min-w-0 flex-wrap items-center gap-1.5">
          <NodeStatusBadge {node} showNodeName />
          {#if node?.is_ephemeral}
            <Badge variant="secondary">{m.session_details_ephemeral()}</Badge>
          {/if}
        </dd>

        <dt class="text-muted-foreground">{m.session_details_working_dir()}</dt>
        <dd class="min-w-0">
          <button
            type="button"
            class="hover:text-foreground flex min-w-0 max-w-full items-center gap-1 text-left font-mono text-[11px] transition-colors"
            title={m.session_working_dir_copy_title({ path: session.local_path })}
            onclick={() => void copy(session.local_path, m.session_working_dir_copied())}
          >
            <span class="truncate">{session.local_path}</span>
            <CopyIcon class="size-3 shrink-0" />
          </button>
        </dd>

        <dt class="text-muted-foreground">{m.session_details_branch()}</dt>
        <dd class="flex min-w-0 flex-wrap items-center gap-1.5">
          <span class="truncate font-mono text-[11px]">{session.git_branch ?? "—"}</span>
          {#if session.is_worktree}
            <Badge variant="secondary">{m.session_details_worktree()}</Badge>
          {/if}
        </dd>

        <dt class="text-muted-foreground">{m.session_details_agent()}</dt>
        <dd class="truncate font-mono text-[11px]">{session.agent_id}</dd>

        {#if session.current_mode}
          <dt class="text-muted-foreground">{m.session_details_mode()}</dt>
          <dd class="font-mono text-[11px]">{session.current_mode}</dd>
        {/if}

        <dt class="text-muted-foreground">{m.session_details_permission()}</dt>
        <dd>{permissionLabel}</dd>

        {#if status}
          <dt class="text-muted-foreground">{m.session_details_status()}</dt>
          <dd>{sessionStatusLabel(status)}</dd>
        {/if}

        <dt class="text-muted-foreground">{m.session_details_context()}</dt>
        <dd>
          {#if usage}<ContextMeter {usage} />{:else}—{/if}
        </dd>

        <dt class="text-muted-foreground">{m.session_details_created()}</dt>
        <dd class="font-mono text-[11px]">{formatEpochMs(session.created_at)}</dd>

        <dt class="text-muted-foreground">{m.session_details_updated()}</dt>
        <dd class="font-mono text-[11px]">{formatEpochMs(session.updated_at)}</dd>

        {#if session.parent_session_id}
          <dt class="text-muted-foreground">{m.session_details_forked_from()}</dt>
          <dd>
            <a
              class="text-primary underline underline-offset-2"
              href={`/sessions/${session.parent_session_id}`}
            >
              {session.parent_session_id.slice(0, 8)}
            </a>
          </dd>
        {/if}

        <dt class="text-muted-foreground">{m.session_details_session_id()}</dt>
        <dd class="flex min-w-0 items-center gap-1">
          <span class="truncate font-mono text-[11px]">{session.session_id}</span>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label={m.session_details_copy_session_id()}
            title={m.session_details_copy_session_id()}
            onclick={() => void copy(session.session_id, m.problem_copied())}
          >
            <CopyIcon class="size-3" />
          </Button>
        </dd>
      </dl>
    </Popover.Content>
  </Popover.Root>
{/if}
