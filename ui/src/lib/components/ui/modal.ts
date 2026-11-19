import { createEffect, type Accessor } from "solid-js";
import { createControllable } from "./controllable.ts";

/**
 * Shared plumbing for the modal wrappers (`Dialog`, `AlertDialog`, `Sheet`).
 *
 * Focus handling belongs to Terracotta: `Dialog` / `AlertDialog` save the focus
 * start point when they open and restore it when they close or unmount. The
 * wrapper only owns the state and the page scroll lock.
 */

/** Open modals sharing the page. Counted so nested dialogs restore correctly. */
let scrollLocks = 0;
let previousOverflow = "";

/** Locks `body` scrolling until the returned function is called. */
export function lockBodyScroll(): () => void {
  if (typeof document === "undefined") return () => {};
  if (scrollLocks === 0) {
    previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
  }
  scrollLocks += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    scrollLocks = Math.max(0, scrollLocks - 1);
    if (scrollLocks === 0) document.body.style.overflow = previousOverflow;
  };
}

/** Locks page scrolling while `open` is true. */
export function useModalScrollLock(open: Accessor<boolean>): void {
  createEffect(open, (isOpen) => {
    if (!isOpen) return;
    return lockBodyScroll();
  });
}

export interface ModalRootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
}

export interface ModalState {
  open: Accessor<boolean>;
  setOpen: (open: boolean) => void;
  close: () => void;
  disabled: Accessor<boolean>;
}

/** Controllable open state plus the scroll lock every modal wrapper shares. */
export function createModalState(props: ModalRootProps): ModalState {
  const [open, setOpen] = createControllable(
    () => props.open,
    () => props.onOpenChange,
    () => props.defaultOpen ?? false,
  );
  useModalScrollLock(open);
  return {
    open,
    setOpen,
    close: () => setOpen(false),
    disabled: () => props.disabled === true,
  };
}
