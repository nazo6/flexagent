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
  value: Accessor<T | undefined>,
  onChange: Accessor<((next: T) => void) | undefined>,
  fallback: Accessor<T>,
): [get: Accessor<T>, set: (next: T) => void] {
  // The supported input is a value or an accessor. Initialise uncontrolled
  // state with the fallback so it can be written back to `undefined`/`null`
  // later without losing the default.
  const [internal, setInternal] = createSignal<T>(untrack(fallback) as Exclude<T, Function>);

  // Presence decides the mode on every read: switching a caller from controlled
  // to uncontrolled (or back) works without remounting the primitive.
  const read = (): T => {
    const external = value();
    return external !== undefined ? external : (internal() ?? fallback());
  };

  const write = (next: T): void => {
    // Duplicate close calls (the wrapper and Terracotta both react to the same
    // click) must not reach the caller twice.
    if (Object.is(read(), next)) return;
    // Keep the last requested value so switching from controlled back to
    // uncontrolled has a sensible value to show.
    setInternal(() => next);
    onChange()?.(next);
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
