import {
  arrow,
  autoUpdate,
  computePosition,
  flip,
  offset,
  shift,
  size,
  type Middleware,
  type Placement,
  type Side,
} from "@floating-ui/dom";
import {
  createContext,
  createEffect,
  createMemo,
  createSignal,
  untrack,
  useContext,
  type Accessor,
} from "solid-js";

/**
 * Anchored floating panels.
 *
 * Terracotta positions nothing: every popup (`Popover`, `Listbox`, `Menu`,
 * `Tooltip`) renders an unstyled element and leaves the coordinates to us.
 * This module is the one place that computes them.
 *
 * A panel is portaled to `document.body` and positioned with
 * `position: fixed`, so it escapes `overflow` clipping and transformed
 * ancestors (the chat timeline and the settings panes both scroll). `flip` and
 * `shift` keep it on screen; `size` clamps its height to the viewport so a long
 * menu scrolls instead of running off the bottom edge.
 */

/** Alignment across the anchor's axis. */
export type PanelAlign = "start" | "center" | "end";

/** A value or an accessor of it, so callers can pass props straight through. */
type Maybe<T> = T | Accessor<T>;

function read<T>(value: Maybe<T> | undefined, fallback: T): T {
  if (value === undefined) return fallback;
  return typeof value === "function" ? (value as Accessor<T>)() : value;
}

/** A DOM ref callback, the only ref shape our wrappers accept. */
export type ElementRef<T extends HTMLElement = HTMLElement> = (element: T) => void;

/** Forwards an element to several ref callbacks (the app's and ours). */
export function composeRef<T extends HTMLElement>(
  ...refs: Array<ElementRef<T> | undefined>
): ElementRef<T> {
  return (element) => {
    for (const ref of refs) ref?.(element);
  };
}

function toPlacement(side: Side, align: PanelAlign): Placement {
  return (align === "center" ? side : `${side}-${align}`) as Placement;
}

function splitPlacement(placement: Placement): [Side, PanelAlign] {
  const [side, align] = placement.split("-") as [Side, PanelAlign | undefined];
  return [side, align ?? "center"];
}

/** The side an arrow sits on, given the side the panel ended up on. */
const ARROW_SIDE: Record<Side, Side> = {
  top: "bottom",
  right: "left",
  bottom: "top",
  left: "right",
};

export interface AnchoredPanelOptions {
  /** The element the panel points at. It may be missing until the trigger mounts. */
  anchor: Accessor<HTMLElement | null | undefined>;
  /** Skip positioning while the panel is hidden (`unmount={false}` panels). */
  open?: Accessor<boolean>;
  side?: Maybe<Side>;
  align?: Maybe<PanelAlign>;
  /** Gap between the anchor and the panel. */
  sideOffset?: Maybe<number>;
  /** Shift along the anchor's edge (negative values pull a submenu back). */
  alignOffset?: Maybe<number>;
  /** Make the panel at least as wide as the anchor (selects match their trigger). */
  matchWidth?: Maybe<boolean>;
  /** Clamp the panel's height to the space left on screen. Defaults to `true`. */
  availableHeight?: Maybe<boolean>;
  /** Keep-out margin from the viewport edges. Defaults to `8`. */
  padding?: Maybe<number>;
  /** Optional arrow element; it is placed against the anchor's edge. */
  arrow?: Accessor<HTMLElement | null | undefined>;
  /** Reports the side the panel settled on, for `data-side` styling. */
  onSideChange?: (side: Side) => void;
}

export interface AnchoredPanel {
  /** Ref for the panel element. */
  ref: ElementRef;
  /** The panel element, once the ref has run. */
  element: Accessor<HTMLElement | undefined>;
  /** The side the panel actually landed on (after `flip`). */
  side: Accessor<Side>;
  /** The alignment the panel actually landed on. */
  align: Accessor<PanelAlign>;
}

/**
 * Keeps a panel anchored to `options.anchor` while it is open.
 *
 * The returned `ref` must be given to the panel element. `autoUpdate` restarts
 * whenever the element or the open state changes, and stops when the panel
 * unmounts, so a closed popup costs nothing.
 */
export function useAnchoredPanel(options: AnchoredPanelOptions): AnchoredPanel {
  const [panel, setPanel] = createSignal<HTMLElement>();
  const [side, setSide] = createSignal<Side>(untrack(() => read(options.side, "bottom")));
  const [align, setAlign] = createSignal<PanelAlign>(untrack(() => read(options.align, "center")));

  /** Geometry inputs; the memo keeps a re-render with equal values from restarting. */
  const state = createMemo(
    () => ({
      element: panel(),
      anchor: options.anchor(),
      open: options.open ? options.open() : true,
      placement: toPlacement(read(options.side, "bottom"), read(options.align, "center")),
      sideOffset: read(options.sideOffset, 0),
      alignOffset: read(options.alignOffset, 0),
      matchWidth: read(options.matchWidth, false),
      availableHeight: read(options.availableHeight, true),
      padding: read(options.padding, 8),
      arrow: options.arrow?.() ?? undefined,
    }),
    {
      equals: (left, right) =>
        left.element === right.element &&
        left.anchor === right.anchor &&
        left.open === right.open &&
        left.placement === right.placement &&
        left.sideOffset === right.sideOffset &&
        left.alignOffset === right.alignOffset &&
        left.matchWidth === right.matchWidth &&
        left.availableHeight === right.availableHeight &&
        left.padding === right.padding &&
        left.arrow === right.arrow,
    },
  );

  createEffect(state, (current) => {
    const { element, anchor } = current;
    if (!element || !anchor || !current.open) return;

    let cancelled = false;
    let stopAutoUpdate: (() => void) | undefined;
    let retries = 0;

    const update = () => {
      void computePosition(anchor, element, {
        strategy: "fixed",
        placement: current.placement,
        middleware: buildMiddleware(current),
      }).then((result) => {
        if (cancelled || !element.isConnected) return;

        element.style.position = "fixed";
        element.style.left = `${Math.round(result.x)}px`;
        element.style.top = `${Math.round(result.y)}px`;
        element.style.visibility = "";

        const [placedSide, placedAlign] = splitPlacement(result.placement);
        element.dataset.side = placedSide;
        element.dataset.align = placedAlign;
        setSide(placedSide);
        setAlign(placedAlign);
        untrack(() => options.onSideChange?.(placedSide));

        const arrowData = result.middlewareData.arrow;
        if (current.arrow && arrowData) positionArrow(current.arrow, placedSide, arrowData);
      });
    };

    // A trigger ref can be stale (for example a re-created slot tree). A
    // detached anchor makes floating-ui walk a different document's tree, so
    // wait for a live element in the panel's document instead.
    const start = () => {
      if (cancelled) return;
      const ready =
        element.isConnected && anchor.isConnected && anchor.ownerDocument === element.ownerDocument;
      if (!ready) {
        if (retries >= 60) return;
        retries += 1;
        requestAnimationFrame(start);
        return;
      }

      // A panel kept mounted while closed (`unmount={false}`) would otherwise
      // paint once at the viewport's top-left before the first measurement.
      element.style.visibility = "hidden";
      // `autoUpdate` performs the first measurement itself.
      stopAutoUpdate = autoUpdate(anchor, element, update);
    };

    start();

    return () => {
      cancelled = true;
      stopAutoUpdate?.();
      element.style.visibility = "";
    };
  });

  return { ref: setPanel, element: panel, side, align };
}

interface PanelState {
  placement: Placement;
  sideOffset: number;
  alignOffset: number;
  matchWidth: boolean;
  availableHeight: boolean;
  padding: number;
  arrow: HTMLElement | undefined;
}

function buildMiddleware(state: PanelState): Middleware[] {
  const middleware: Middleware[] = [
    offset({ mainAxis: state.sideOffset, crossAxis: state.alignOffset }),
    flip({ padding: state.padding }),
    shift({ padding: state.padding, crossAxis: true }),
  ];

  if (state.arrow) middleware.push(arrow({ element: state.arrow }));

  if (state.matchWidth || state.availableHeight) {
    middleware.push(
      size({
        padding: state.padding,
        apply({ rects, availableHeight, elements }) {
          // `minWidth` rather than `width`: a caller's `w-80` still wins.
          if (state.matchWidth) {
            elements.floating.style.minWidth = `${Math.round(rects.reference.width)}px`;
          }
          if (state.availableHeight) {
            elements.floating.style.maxHeight = `${Math.round(availableHeight)}px`;
          }
        },
      }),
    );
  }

  return middleware;
}

function positionArrow(
  element: HTMLElement,
  side: Side,
  data: { x?: number; y?: number; centerOffset?: number },
): void {
  const staticSide = ARROW_SIDE[side];
  element.style.position = "absolute";
  element.dataset.side = staticSide;
  // Clear every inset first: a `flip()` between placements would otherwise
  // leave the previous static side set and push the arrow off the panel.
  element.style.left = "";
  element.style.right = "";
  element.style.top = "";
  element.style.bottom = "";
  element.style.left = data.x != null ? `${Math.round(data.x)}px` : "";
  element.style.top = data.y != null ? `${Math.round(data.y)}px` : "";
  element.style[staticSide] = "-4px";
}

/**
 * Marks an element as one of our portaled panels.
 *
 * Outside-pointer dismissal ignores pointer events inside any of them: a menu
 * item in a submenu lives in a second portal, and clicking it must not dismiss
 * the menu it was opened from.
 */
export const ANCHORED_PANEL_ATTRIBUTE = "data-anchored-panel";

/**
 * Ownership tree of the portaled panels.
 *
 * A nested panel (a `Select` inside a `Popover`, a submenu inside a menu) is a
 * separate portal, so `event.target.closest()` cannot see that it belongs to
 * the outer panel. Each `Content` creates a node and registers its own element;
 * dismissal walks from the innermost panel's node to the outer ones.
 */
export interface PanelTree {
  parent: PanelTree | null;
  element: HTMLElement | null;
}

export const PanelTreeContext = createContext<PanelTree | null>(null);

const panelOwners = new WeakMap<Element, PanelTree>();

/** Creates the tree node for one panel, nested under the nearest provider. */
export function createPanelTree(): PanelTree {
  return { parent: useContext(PanelTreeContext), element: null };
}

/** Registers the panel element that owns this tree node. */
export function registerPanelElement(tree: PanelTree, element: HTMLElement): void {
  tree.element = element;
  panelOwners.set(element, tree);
}

/** Whether `element` is the panel of `tree` or of one of its nested panels. */
function belongsToTree(element: Element, tree: PanelTree): boolean {
  let current: PanelTree | null = panelOwners.get(element) ?? null;
  while (current !== null) {
    if (current === tree) return true;
    current = current.parent;
  }
  return false;
}

export interface DismissOptions {
  open: Accessor<boolean>;
  /** The trigger; pressing it must not dismiss (it toggles). */
  anchor?: Accessor<HTMLElement | null | undefined>;
  /** Ownership tree of the panel, for presses inside nested panels. */
  tree?: PanelTree;
  onDismiss: () => void;
}

/**
 * Calls `onDismiss` on a pointer press outside every anchored panel.
 *
 * Terracotta closes its popups when focus leaves the panel, which misses a
 * press on something that cannot take focus (a blank area of the page). The
 * desktop UI expects that press to close the popup, so the panels add this.
 */
export function dismissOnOutsidePointer(options: DismissOptions): void {
  const handle = (event: PointerEvent) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const panel = target.closest(`[${ANCHORED_PANEL_ATTRIBUTE}]`);
    if (panel !== null) {
      // A nested panel under this one keeps it open; an unrelated panel does not.
      if (options.tree === undefined || belongsToTree(panel, options.tree)) return;
    }
    if (options.anchor?.()?.contains(target)) return;
    options.onDismiss();
  };

  createEffect(options.open, (open) => {
    if (!open) return;
    document.addEventListener("pointerdown", handle, true);
    return () => document.removeEventListener("pointerdown", handle, true);
  });
}
