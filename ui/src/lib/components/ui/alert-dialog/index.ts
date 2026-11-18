import {
  Action,
  Cancel,
  Content,
  Description,
  Footer,
  Header,
  Media,
  Overlay,
  PortalRoot,
  Root,
  Title,
  Trigger,
} from "./alert-dialog.tsx";

export {
  Root,
  Trigger,
  PortalRoot as Portal,
  Overlay,
  Content,
  Header,
  Footer,
  Title,
  Description,
  Action,
  Cancel,
  Media,
};

/** Namespace shape, so `import * as AlertDialog` keeps working unchanged. */
export const AlertDialog = {
  Root,
  Trigger,
  Portal: PortalRoot,
  Overlay,
  Content,
  Header,
  Footer,
  Title,
  Description,
  Action,
  Cancel,
  Media,
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
  ActionProps,
  CancelProps,
  MediaProps,
} from "./alert-dialog.tsx";
