import { Content, List, Root, Trigger } from "./tabs.tsx";

export {
  Root,
  Content,
  List,
  Trigger,
  tabsListVariants,
  type TabsListVariant,
  type RootProps,
  type ListProps,
  type TriggerProps,
  type ContentProps,
} from "./tabs.tsx";

/** Namespace shape, so `import * as Tabs` keeps working unchanged. */
export const Tabs = { Root, Content, List, Trigger };
