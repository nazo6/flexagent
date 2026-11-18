import { createContext, useContext } from "solid-js";

/**
 * State shared by the parts of a popover.
 *
 * Terracotta's `Popover` owns the open state; the trigger and the panel need to
 * share the trigger element so the panel can be positioned against it.
 */
export interface PopoverState {
  open: () => boolean;
  setOpen: (open: boolean) => void;
  anchor: () => HTMLElement | undefined;
  setAnchor: (element: HTMLElement) => void;
}

export const PopoverContext = createContext<PopoverState | null>(null);

export function usePopoverState(component: string): PopoverState {
  const state = useContext(PopoverContext);
  if (!state) throw new Error(`<${component}> must be used inside a <Popover.Root>`);
  return state;
}
