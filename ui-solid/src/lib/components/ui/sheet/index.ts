import {
  Close,
  Content,
  Description,
  Footer,
  Header,
  Overlay,
  PortalRoot,
  Root,
  Title,
  Trigger,
} from "./sheet.tsx";

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
  Close,
};

/** Namespace shape, so `import * as Sheet` keeps working unchanged. */
export const Sheet = {
  Root,
  Trigger,
  Portal: PortalRoot,
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
} from "./sheet.tsx";
