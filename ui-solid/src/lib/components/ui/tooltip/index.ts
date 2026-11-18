import { Content, PortalRoot, Provider, Root, Trigger } from "./tooltip.tsx";

export { Root, Trigger, Content, Provider, PortalRoot as Portal };

/** Namespace shape, so `import * as Tooltip` keeps working unchanged. */
export const Tooltip = { Root, Trigger, Content, Provider, Portal: PortalRoot };

export type {
  RootProps,
  TriggerProps,
  ContentProps,
  ProviderProps,
  PortalProps,
} from "./tooltip.tsx";
