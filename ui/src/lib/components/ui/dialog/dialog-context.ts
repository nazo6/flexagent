import { createContext, useContext, type Context } from "solid-js";

/**
 * State shared by the parts of a dialog-family root.
 *
 * Terracotta's `Dialog` owns the open state and each of its parts reads it
 * through Terracotta's own context; the trigger and close button are ours, so
 * they need a handle on the same state. Keeping it in our context also means
 * the trigger does not have to be a `DialogPanel` descendant.
 */
export interface DialogState {
  open: () => boolean;
  setOpen: (open: boolean) => void;
  close: () => void;
}

export type DialogContext = Context<DialogState | null>;

/** One context per primitive, so a dialog and an alert dialog never mix. */
export function makeDialogContext(): DialogContext {
  return createContext<DialogState | null>(null);
}

export function useDialogState(context: DialogContext, component: string): DialogState {
  const state = useContext(context);
  if (!state) throw new Error(`<${component}> must be used inside its root`);
  return state;
}
