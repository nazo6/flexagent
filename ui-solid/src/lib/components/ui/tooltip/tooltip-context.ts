import { createContext, useContext } from "solid-js";

/** Delay shared by every tooltip below a `Tooltip.Provider`. */
export interface TooltipConfig {
  delayDuration: () => number;
}

export const TooltipConfigContext = createContext<TooltipConfig | null>(null);

export function useTooltipConfig(): number {
  return useContext(TooltipConfigContext)?.delayDuration() ?? 0;
}
