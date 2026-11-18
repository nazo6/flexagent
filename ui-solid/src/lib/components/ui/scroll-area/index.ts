import { Corner, Root, Scrollbar, Thumb, Viewport } from "./scroll-area.tsx";

export {
  Root,
  Viewport,
  Scrollbar,
  Thumb,
  Corner,
  type RootProps,
  type ViewportProps,
  type ScrollbarProps,
  type ThumbProps,
  type CornerProps,
  type ScrollAreaOrientation,
} from "./scroll-area.tsx";

/** Namespace shape, so `import * as ScrollArea` keeps working unchanged. */
export const ScrollArea = { Root, Viewport, Scrollbar, Thumb, Corner };
