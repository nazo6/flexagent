import { createMemo, For, Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import * as Sheet from "#lib/components/ui/sheet/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { dataSafetyMessage } from "#lib/api/data-safety.ts";
import { troubleshootingUrl } from "#lib/api/docs.ts";
import { recoveryActionLabel } from "#lib/api/recovery.ts";
import type { RecoveryAction } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { problems, type AppProblem } from "#lib/stores/problems.ts";
import { cn } from "#lib/utils.ts";
import IconCircleAlert from "~icons/lucide/circle-alert";
import IconCopy from "~icons/lucide/copy";
import IconTrash2 from "~icons/lucide/trash-2";
import IconTriangleAlert from "~icons/lucide/triangle-alert";
import IconX from "~icons/lucide/x";

/** Copies the server correlation ID shown in the problem details. */
async function copyRequestId(problem: AppProblem): Promise<void> {
  if (problem.requestId === null) return;
  try {
    await navigator.clipboard.writeText(problem.requestId);
    toast.success(m.problem_copied());
  } catch (error) {
    problems.report("copy request id", error);
  }
}

/**
 * Problem center, ported from `ui/src/lib/components/ProblemCenter.svelte`.
 *
 * Persistent (unlike toasts) list of failures with details, documentation links
 * and server-provided recovery actions.
 */

export interface ProblemCenterProps {
  /** Called when the user chooses a recovery action (when provided). */
  onAction?: (problem: AppProblem, action: RecoveryAction) => void;
}

export default function ProblemCenter(props: ProblemCenterProps) {
  return (
    <Sheet.Root
      open={problems.centerOpen}
      onOpenChange={(open) => {
        problems.centerOpen = open;
      }}
    >
      <Sheet.Content side="right" class="w-full sm:max-w-md">
        <Sheet.Header>
          <Sheet.Title class="flex items-center gap-2">
            <IconTriangleAlert class="size-4" />
            {m.problem_center_title()}
            <Show when={problems.problems.length > 0}>
              <span class="text-muted-foreground font-mono text-[11px]">
                {problems.problems.length}
              </span>
            </Show>
          </Sheet.Title>
          <Sheet.Description>{m.problem_center_description()}</Sheet.Description>
        </Sheet.Header>

        <div class="app-scroll flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-4 pb-4">
          <Show
            when={problems.problems.length > 0}
            fallback={
              <p class="text-muted-foreground py-6 text-center text-[13px]">{m.problem_empty()}</p>
            }
          >
            <For each={problems.problems}>
              {(problem) => (
                <ProblemCard
                  problem={problem}
                  onAction={props.onAction}
                  onCopy={() => void copyRequestId(problem)}
                />
              )}
            </For>
          </Show>
        </div>

        <Show when={problems.problems.length > 0}>
          <div class="border-t border-border p-3">
            <Button
              variant="outline"
              size="sm"
              class="w-full text-destructive hover:bg-destructive/10 hover:text-destructive"
              onClick={() => problems.clear()}
            >
              <IconTrash2 class="size-3.5" />
              {m.problem_clear_all()}
            </Button>
          </div>
        </Show>
      </Sheet.Content>
    </Sheet.Root>
  );
}

interface ProblemCardProps {
  problem: AppProblem;
  onAction?: (problem: AppProblem, action: RecoveryAction) => void;
  onCopy: () => void;
}

function ProblemCard(props: ProblemCardProps) {
  const safety = createMemo(() => dataSafetyMessage(props.problem.dataSafety));
  const docsUrl = createMemo(
    () => props.problem.docs ?? troubleshootingUrl(props.problem.code) ?? "#",
  );

  return (
    <div
      class={cn(
        "flex flex-col gap-2 rounded-lg border p-3 text-[13px]",
        props.problem.severity === "error" ? "border-danger/30 bg-card" : "border-warn/30 bg-card",
      )}
    >
      <div class="flex items-start gap-2">
        <Show
          when={props.problem.severity === "error"}
          fallback={<IconTriangleAlert class="text-warn mt-0.5 size-4 shrink-0" />}
        >
          <IconCircleAlert class="text-danger mt-0.5 size-4 shrink-0" />
        </Show>
        <div class="min-w-0 flex-1">
          <p class="min-w-0 break-words">
            <span class="text-muted-foreground">{props.problem.context}:</span>{" "}
            {props.problem.message}
          </p>
          <Show when={safety() !== null}>
            <p class="text-muted-foreground mt-0.5 text-[11px]">{safety()}</p>
          </Show>
        </div>
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={m.problem_dismiss()}
          title={m.problem_dismiss()}
          onClick={() => problems.dismiss(props.problem.id)}
        >
          <IconX class="size-3.5" />
        </Button>
      </div>

      <details class="group">
        <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px]">
          {m.problem_details()}
        </summary>
        <div class="mt-1 flex flex-col gap-1 text-[13px]">
          <Show when={props.problem.code !== null}>
            <p class="text-muted-foreground font-mono">{props.problem.code}</p>
          </Show>
          <Show when={props.problem.requestId !== null}>
            <p class="text-muted-foreground font-mono break-all">
              {m.problem_request_id({ id: props.problem.requestId ?? "" })}
            </p>
          </Show>
          <Show when={props.problem.details !== null}>
            <pre class="bg-muted/50 max-h-48 overflow-auto rounded p-2 font-mono whitespace-pre-wrap break-words">
              {props.problem.details}
            </pre>
          </Show>
          <Show when={props.problem.docs !== null || props.problem.code !== null}>
            <a
              class="text-primary underline underline-offset-2"
              href={docsUrl()}
              target="_blank"
              rel="noreferrer"
            >
              {m.problem_docs()}
            </a>
          </Show>
        </div>
      </details>

      <div class="flex flex-wrap items-center justify-end gap-1">
        <For each={props.problem.recovery}>
          {(action) => (
            <Button
              size="xs"
              variant="outline"
              onClick={() => props.onAction?.(props.problem, action)}
              disabled={props.onAction === undefined}
            >
              {recoveryActionLabel(action)}
            </Button>
          )}
        </For>
        <Show when={props.problem.requestId !== null}>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label={m.problem_copy_request_id()}
            title={m.problem_copy_request_id()}
            onClick={props.onCopy}
          >
            <IconCopy class="size-3.5" />
          </Button>
        </Show>
      </div>
    </div>
  );
}
