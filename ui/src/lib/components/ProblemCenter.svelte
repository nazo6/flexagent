<script lang="ts">
  import * as Sheet from "#lib/components/ui/sheet/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import type { RecoveryAction } from "#lib/generated/bindings.ts";
  import { dataSafetyMessage } from "#lib/api/data-safety.ts";
  import { troubleshootingUrl } from "#lib/api/docs.ts";
  import { recoveryActionKey, recoveryActionLabel } from "#lib/api/recovery.ts";
  import { problems, type AppProblem } from "#lib/stores/problems.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { cn } from "#lib/utils.ts";
  import { toast } from "svelte-sonner";
  import CircleAlertIcon from "@lucide/svelte/icons/circle-alert";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";
  import XIcon from "@lucide/svelte/icons/x";

  interface Props {
    /** Called when the user chooses a recovery action (when provided). */
    onaction?: (problem: AppProblem, action: RecoveryAction) => void;
  }

  let { onaction }: Props = $props();

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

<Sheet.Root bind:open={problems.centerOpen}>
  <Sheet.Content side="right" class="w-full sm:max-w-md">
    <Sheet.Header>
      <Sheet.Title class="flex items-center gap-2">
        <TriangleAlertIcon class="size-4" />
        {m.problem_center_title()}
        {#if problems.problems.length > 0}
          <span class="text-muted-foreground font-mono text-[11px]">
            {problems.problems.length}
          </span>
        {/if}
      </Sheet.Title>
      <Sheet.Description>{m.problem_center_description()}</Sheet.Description>
    </Sheet.Header>

    <div class="app-scroll flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-4 pb-4">
      {#if problems.problems.length === 0}
        <p class="text-muted-foreground py-6 text-center text-[13px]">{m.problem_empty()}</p>
      {:else}
        {#each problems.problems as problem (problem.id)}
          <div
            class={cn(
              "flex flex-col gap-2 rounded-lg border p-3 text-[13px]",
              problem.severity === "error" ? "border-danger/30 bg-card" : "border-warn/30 bg-card",
            )}
          >
            <div class="flex items-start gap-2">
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
                  <p class="text-muted-foreground mt-0.5 text-[11px]">
                    {dataSafetyMessage(problem.dataSafety)}
                  </p>
                {/if}
              </div>
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

            <details class="group">
              <summary
                class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px]"
              >
                {m.problem_details()}
              </summary>
              <div class="mt-1 flex flex-col gap-1 text-[13px]">
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

            <div class="flex flex-wrap items-center justify-end gap-1">
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
            </div>
          </div>
        {/each}
      {/if}
    </div>

    {#if problems.problems.length > 0}
      <div class="border-t border-border p-3">
        <Button
          variant="outline"
          size="sm"
          class="w-full text-destructive hover:bg-destructive/10 hover:text-destructive"
          onclick={() => problems.clear()}
        >
          <Trash2Icon class="size-3.5" />
          {m.problem_clear_all()}
        </Button>
      </div>
    {/if}
  </Sheet.Content>
</Sheet.Root>
