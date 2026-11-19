import {
  Close,
  Content,
  Description,
  Footer,
  Header,
  NestedRoot,
  Overlay,
  Portal,
  Root,
  Title,
  Trigger,
} from "./drawer.tsx";

export {
  Root,
  NestedRoot,
  Trigger,
  Portal,
  Overlay,
  Content,
  Header,
  Footer,
  Title,
  Description,
  Close,
} from "./drawer.tsx";

/** Namespace shape, so `import * as Drawer` keeps working unchanged. */
export const Drawer = {
  Root,
  NestedRoot,
  Trigger,
  Portal,
  Overlay,
  Content,
  Header,
  Footer,
  Title,
  Description,
  Close,
};

export type {
  RootProps,
  TriggerProps,
  PortalProps,
  OverlayProps,
  ContentProps,
  HeaderProps,
  FooterProps,
  TitleProps,
  DescriptionProps,
  CloseProps,
  Side,
} from "./drawer.tsx";
