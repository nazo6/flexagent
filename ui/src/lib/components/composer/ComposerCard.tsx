import type { JSX } from "@solidjs/web";
import { children, Show } from "solid-js";
import { cn } from "#lib/utils.ts";

export interface ComposerCardProps {
  /** Content above the input (for example workspace pills or notices). */
  above?: JSX.Element;
  /**
   * Selectors below the input (agent / mode / model / other options).
   *
   * The row scrolls horizontally instead of wrapping, so the action cluster on
   * the right never moves.
   */
  selectors?: JSX.Element;
  /** Actions below the input, pinned to the right and never wrapped. */
  actions?: JSX.Element;
  class?: string;
  /** The input area (usually a `ComposerTextarea`). */
  children?: JSX.Element;
}

/** Frame of a composer: notices, the input, and the selector / action row. */
export default function ComposerCard(props: ComposerCardProps) {
  // Reading a JSX-element prop creates its tree. Passing it to both `when` and
  // the inserted content (or comparing it with `undefined`) would create a
  // second, detached tree whose effects still run (popovers, portals). Resolve
  // each slot once and render that value.
  const above = children(() => props.above);
  const selectors = children(() => props.selectors);
  const actions = children(() => props.actions);

  return (
    <div
      class={cn(
        "bg-card border-border focus-within:border-signal/50 focus-within:ring-signal/15 rounded-lg border p-2 transition-[border-color,box-shadow] focus-within:ring-2",
        props.class,
      )}
    >
      {above()}
      {props.children}
      <Show when={selectors() !== undefined || actions() !== undefined}>
        <div class="border-border flex items-center gap-1.5 border-t px-0.5 pt-1">
          <div class="relative min-w-0 flex-1">
            <div class="flex items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
              {selectors()}
            </div>
            <div class="from-card pointer-events-none absolute inset-y-0 right-0 w-8 bg-gradient-to-l to-transparent" />
          </div>
          <div class="flex shrink-0 items-center gap-1">{actions()}</div>
        </div>
      </Show>
    </div>
  );
}
