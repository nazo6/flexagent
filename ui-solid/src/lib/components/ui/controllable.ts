import { createSignal, untrack, type Accessor } from "solid-js";

/**
 * Prop plumbing for the ported primitives.
 *
 * Two things every wrapper needs:
 *
 * - **Controllable props without two-way binding.** A Svelte caller wrote
 *   `bind:open` / `bind:value`; the Solid port passes the value and the change
 *   callback explicitly. Both call shapes must work: `<Popover.Root>` (no
 *   props) and `<Popover.Root open={open()} onOpenChange={setOpen}>`.
 * - **Plain event handler types.** Solid's `EventHandlerUnion` also allows a
 *   `[handler, data]` tuple, which cannot be called. Wrappers that compose a
 *   handler with their own behaviour need the callable form.
 */

export function createControllable<T>(
  value: () => T | undefined,
  onChange: ((next: T) => void) | undefined,
  fallback: T,
): [get: Accessor<T>, set: (next: T) => void] {
  const [internal, setInternal] = createSignal<T>();
  // Presence decides the mode once; reading it at setup is not a dependency.
  const controlled = untrack(() => value() !== undefined);

  const read = (): T => (controlled ? (value() ?? fallback) : (internal() ?? fallback));

  const write = (next: T): void => {
    if (!controlled) setInternal(() => next);
    onChange?.(next);
  };

  return [read, write];
}

/**
 * Reads a DOM boolean attribute (`true`, `false` or `""`) as a boolean.
 *
 * `ComponentProps<"button">["disabled"]` allows `""`; Terracotta's props want a
 * plain boolean, so every wrapper that forwards a native button normalises it.
 */
export function toBoolean(value: boolean | "" | undefined): boolean {
  return value === true || value === "";
}

/** A mouse handler that knows its element, as Solid infers it. */
export type MouseHandler<T extends Element> = (
  event: MouseEvent & { currentTarget: T; target: Element },
) => void;

/** An input handler that knows its field, as Solid infers it. */
export type InputHandler<T extends HTMLInputElement | HTMLTextAreaElement> = (
  event: InputEvent & { currentTarget: T; target: Element },
) => void;
