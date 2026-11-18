import { m } from "#lib/paraglide/messages.js";
import { problems } from "#lib/stores/problems.ts";
import { cn } from "#lib/utils.ts";
import IconCircleAlert from "~icons/lucide/circle-alert";
import IconTriangleAlert from "~icons/lucide/triangle-alert";
import { Show } from "solid-js";

export interface ProblemBadgeProps {
  /** Show the text label next to the count. */
  label?: boolean;
  class?: string;
}

/** Compact problem indicator that opens the problem center. */
export default function ProblemBadge(props: ProblemBadgeProps) {
  const count = () => problems.problems.length;
  const hasError = () => problems.problems.some((problem) => problem.severity === "error");

  return (
    <Show when={count() > 0}>
      <button
        type="button"
        class={cn(
          "inline-flex items-center justify-center gap-1.5 rounded-md border px-2 py-1 text-[11px] font-medium transition-colors",
          hasError()
            ? "border-danger/30 bg-danger-soft/60 text-danger hover:bg-danger-soft"
            : "border-warn/30 bg-warn-soft/60 text-warn hover:bg-warn-soft",
          props.class ?? "",
        )}
        title={m.problem_banner_label()}
        aria-label={`${m.problem_banner_label()} (${count()})`}
        onClick={() => problems.openCenter()}
      >
        {hasError() ? (
          <IconCircleAlert class="size-3.5 shrink-0" />
        ) : (
          <IconTriangleAlert class="size-3.5 shrink-0" />
        )}
        <span class="font-mono leading-none">{count()}</span>
        <Show when={props.label}>
          <span class="truncate">{m.problem_banner_label()}</span>
        </Show>
      </button>
    </Show>
  );
}
