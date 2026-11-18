import { Content, PortalRoot, Root, Trigger } from "./popover.tsx";

export { Root, Trigger, PortalRoot as Portal, Content };

/** Namespace shape, so `import * as Popover` keeps working unchanged. */
export const Popover = { Root, Trigger, Portal: PortalRoot, Content };

export type { RootProps, TriggerProps, PortalProps, ContentProps } from "./popover.tsx";
