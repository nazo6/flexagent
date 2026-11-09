<script lang="ts">
  import type { RecoveryAction } from "#lib/generated/bindings.ts";
  import { dataSafetyMessage } from "#lib/api/data-safety.ts";
  import { troubleshootingUrl } from "#lib/api/docs.ts";
  import { recoveryActionKey, recoveryActionLabel } from "#lib/api/recovery.ts";
  import { problems, type AppProblem } from "#lib/stores/problems.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import CircleAlertIcon from "@lucide/svelte/icons/circle-alert";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    /** Called when the user chooses a recovery action (when provided). */
    onaction?: (problem: AppProblem, action: RecoveryAction) => void;
  }

  let { onaction }: Props = $props();

  /** At most this many problems are shown; older ones stay available in the store. */
  const VISIBLE_LIMIT = 3;

  const visible = $derived(problems.problems.slice(0, VISIBLE_LIMIT));
  const hiddenCount = $derived(Math.max(0, problems.problems.length - VISIBLE_LIMIT));

  async function copyRequestId(problem: AppProblem): Promise<void> {
    if (problem.requestId === null) return;
    try {
      await navigator.clipboard.writeText(problem.requestId);
      toast.success(m.problem_copied());
    } catch (error) {
      problems.report("copy request id", error);
    }
  }
</script>

{#if visible.length > 0}
  <section
    class="border-b border-border bg-danger-soft/25 px-4 py-2"
    aria-live="polite"
    aria-label={m.problem_banner_label()}
  >
    <div class="flex flex-col gap-2">
      {#each visible as problem (problem.id)}
        <div
          class={cn(
            "flex items-start gap-2 rounded-lg border px-3 py-2 text-ui",
            problem.severity === "error" ? "border-danger/30 bg-card" : "border-warn/30 bg-card",
          )}
        >
          {#if problem.severity === "error"}
            <CircleAlertIcon class="text-danger mt-0.5 size-4 shrink-0" />
          {:else}
            <TriangleAlertIcon class="text-warn mt-0.5 size-4 shrink-0" />
          {/if}

          <div class="min-w-0 flex-1">
            <p class="min-w-0 break-words">
              <span class="text-muted-foreground">{problem.context}:</span>
              {problem.message}
            </p>

            {#if dataSafetyMessage(problem.dataSafety) !== null}
              <p class="text-muted-foreground mt-0.5 text-2xs">
                {dataSafetyMessage(problem.dataSafety)}
              </p>
            {/if}

            <details class="group mt-1">
              <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-2xs">
                {m.problem_details()}
              </summary>
              <div class="mt-1 flex flex-col gap-1 text-ui">
                {#if problem.code !== null}
                  <p class="text-muted-foreground font-mono">{problem.code}</p>
                {/if}
                {#if problem.requestId !== null}
                  <p class="text-muted-foreground font-mono break-all">
                    {m.problem_request_id({ id: problem.requestId })}
                  </p>
                {/if}
                {#if problem.details !== null}
                  <pre
                    class="bg-muted/50 max-h-48 overflow-auto rounded p-2 font-mono whitespace-pre-wrap break-words">{problem.details}</pre>
                {/if}
                {#if problem.docs !== null || problem.code !== null}
                  <a
                    class="text-primary underline underline-offset-2"
                    href={problem.docs ?? troubleshootingUrl(problem.code) ?? "#"}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {m.problem_docs()}
                  </a>
                {/if}
              </div>
            </details>
          </div>

          <div class="flex shrink-0 flex-wrap items-center justify-end gap-1">
            {#each problem.recovery as action (recoveryActionKey(action))}
              <Button
                size="xs"
                variant="outline"
                onclick={() => onaction?.(problem, action)}
                disabled={onaction === undefined}
              >
                {recoveryActionLabel(action)}
              </Button>
            {/each}
            {#if problem.requestId !== null}
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={m.problem_copy_request_id()}
                title={m.problem_copy_request_id()}
                onclick={() => void copyRequestId(problem)}
              >
                <CopyIcon class="size-3.5" />
              </Button>
            {/if}
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={m.problem_dismiss()}
              title={m.problem_dismiss()}
              onclick={() => problems.dismiss(problem.id)}
            >
              <XIcon class="size-3.5" />
            </Button>
          </div>
        </div>
      {/each}

      {#if hiddenCount > 0}
        <p class="text-muted-foreground text-ui">
          {m.problem_more({ count: String(hiddenCount) })}
        </p>
      {/if}
    </div>
  </section>
{/if}
