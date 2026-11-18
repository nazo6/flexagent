import {
  Content,
  Group,
  GroupHeading,
  Item,
  Label,
  PortalRoot,
  Root,
  ScrollDownButton,
  ScrollUpButton,
  Separator,
  Trigger,
  Value,
} from "./select.tsx";

export {
  Root,
  Trigger,
  Value,
  Content,
  Item,
  Group,
  GroupHeading,
  Label,
  Separator,
  ScrollUpButton,
  ScrollDownButton,
  PortalRoot as Portal,
};

/** Namespace shape, so `import * as Select` keeps working unchanged. */
export const Select = {
  Root,
  Trigger,
  Value,
  Content,
  Item,
  Group,
  GroupHeading,
  Label,
  Separator,
  ScrollUpButton,
  ScrollDownButton,
  Portal: PortalRoot,
};

export type {
  RootProps,
  TriggerProps,
  ValueProps,
  PortalProps,
  ContentProps,
  ItemProps,
  GroupProps,
  GroupHeadingProps,
  LabelProps,
  SeparatorProps,
  ScrollButtonProps,
} from "./select.tsx";
