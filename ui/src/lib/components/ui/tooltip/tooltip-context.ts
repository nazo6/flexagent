import { createContext } from "solid-js";

/** Delay shared by every tooltip below a `Tooltip.Provider`. */
export interface TooltipConfig {
  delayDuration: () => number;
}

export const TooltipConfigContext = createContext<TooltipConfig | null>(null);
