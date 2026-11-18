import { Show } from "solid-js";
import * as Popover from "#lib/components/ui/popover/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import ContextMeter from "#lib/components/session/ContextMeter.tsx";
import NodeStatusBadge from "#lib/components/NodeStatusBadge.tsx";
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
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconCopy from "~icons/lucide/copy";

export interface SessionInfoProps {
  session: SessionSummary | null;
  node: NodeSummary | null;
  status: SessionStatus | null;
  usage: SessionUsage | null;
  projectName: string | null;
  class?: string;
}

/** Node / branch chip plus the full session details popover. */
export default function SessionInfo(props: SessionInfoProps) {
  const permissionLabel = () =>
    props.session?.permission_policy === "auto_allow_all"
      ? m.session_details_permission_auto()
      : m.session_details_permission_ask();

  async function copy(value: string, message: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(message);
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }

  return (
    <Show when={props.session}>
      {(session) => (
        <Popover.Root>
          <Popover.Trigger
            class={cn(
              "text-muted-foreground hover:text-foreground hover:bg-accent focus-visible:ring-ring/40 inline-flex min-w-0 items-center gap-1.5 rounded-md px-1.5 py-1 text-[11px] outline-none transition-colors focus-visible:ring-2",
              props.class,
            )}
            title={m.session_details_title()}
          >
            <NodeStatusBadge node={props.node} showNodeName showLabel={false} />
            <Show when={session().git_branch}>
              {(branch) => (
                <>
                  <span class="opacity-40">·</span>
                  <span class="min-w-0 truncate font-mono">{branch()}</span>
                </>
              )}
            </Show>
            <IconChevronDown class="size-3 shrink-0" />
          </Popover.Trigger>

          <Popover.Content align="start" class="w-[min(23rem,calc(100vw-2rem))] p-0">
            <div class="border-border border-b px-3 py-2">
              <h2 class="text-[13px] font-semibold">{m.session_details_title()}</h2>
            </div>

            <dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2.5 p-3 text-[12px]">
              <dt class="text-muted-foreground">{m.session_details_project()}</dt>
              <dd class="min-w-0 break-words">
                {props.projectName ?? session().project_id}
                <Show when={props.projectName}>
                  <span class="text-muted-foreground block truncate font-mono text-[11px]">
                    {session().project_id}
                  </span>
                </Show>
              </dd>

              <dt class="text-muted-foreground">{m.session_details_node()}</dt>
              <dd class="flex min-w-0 flex-wrap items-center gap-1.5">
                <NodeStatusBadge node={props.node} showNodeName />
                <Show when={props.node?.is_ephemeral}>
                  <Badge variant="secondary">{m.session_details_ephemeral()}</Badge>
                </Show>
              </dd>

              <dt class="text-muted-foreground">{m.session_details_working_dir()}</dt>
              <dd class="min-w-0">
                <button
                  type="button"
                  class="hover:text-foreground flex max-w-full min-w-0 items-center gap-1 text-left font-mono text-[11px] transition-colors"
                  title={m.session_working_dir_copy_title({ path: session().local_path })}
                  onClick={() =>
                    void copy(session().local_path, m.session_working_dir_copied())
                  }
                >
                  <span class="truncate">{session().local_path}</span>
                  <IconCopy class="size-3 shrink-0" />
                </button>
              </dd>

              <dt class="text-muted-foreground">{m.session_details_branch()}</dt>
              <dd class="flex min-w-0 flex-wrap items-center gap-1.5">
                <span class="truncate font-mono text-[11px]">{session().git_branch ?? "—"}</span>
                <Show when={session().is_worktree}>
                  <Badge variant="secondary">{m.session_details_worktree()}</Badge>
                </Show>
              </dd>

              <dt class="text-muted-foreground">{m.session_details_agent()}</dt>
              <dd class="truncate font-mono text-[11px]">{session().agent_id}</dd>

              <Show when={session().current_mode}>
                {(mode) => (
                  <>
                    <dt class="text-muted-foreground">{m.session_details_mode()}</dt>
                    <dd class="font-mono text-[11px]">{mode()}</dd>
                  </>
                )}
              </Show>

              <dt class="text-muted-foreground">{m.session_details_permission()}</dt>
              <dd>{permissionLabel()}</dd>

              <Show when={props.status}>
                {(status) => (
                  <>
                    <dt class="text-muted-foreground">{m.session_details_status()}</dt>
                    <dd>{sessionStatusLabel(status())}</dd>
                  </>
                )}
              </Show>

              <dt class="text-muted-foreground">{m.session_details_context()}</dt>
              <dd>
                <Show when={props.usage} fallback="—">
                  {(usage) => <ContextMeter usage={usage()} />}
                </Show>
              </dd>

              <dt class="text-muted-foreground">{m.session_details_created()}</dt>
              <dd class="font-mono text-[11px]">{formatEpochMs(session().created_at)}</dd>

              <dt class="text-muted-foreground">{m.session_details_updated()}</dt>
              <dd class="font-mono text-[11px]">{formatEpochMs(session().updated_at)}</dd>

              <Show when={session().parent_session_id}>
                {(parent) => (
                  <>
                    <dt class="text-muted-foreground">{m.session_details_forked_from()}</dt>
                    <dd>
                      <a
                        class="text-primary underline underline-offset-2"
                        href={`/sessions/${parent()}`}
                      >
                        {parent().slice(0, 8)}
                      </a>
                    </dd>
                  </>
                )}
              </Show>

              <dt class="text-muted-foreground">{m.session_details_session_id()}</dt>
              <dd class="flex min-w-0 items-center gap-1">
                <span class="truncate font-mono text-[11px]">{session().session_id}</span>
                <Button
                  size="icon-xs"
                  variant="ghost"
                  aria-label={m.session_details_copy_session_id()}
                  title={m.session_details_copy_session_id()}
                  onClick={() => void copy(session().session_id, m.problem_copied())}
                >
                  <IconCopy class="size-3" />
                </Button>
              </dd>
            </dl>
          </Popover.Content>
        </Popover.Root>
      )}
    </Show>
  );
}
