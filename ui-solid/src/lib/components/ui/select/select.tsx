import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { createContext, createEffect, createSignal, omit, onSettled, useContext } from "solid-js";
import { Listbox, ListboxButton, ListboxOption, ListboxOptions } from "terracotta/listbox";
import { cn } from "#lib/utils.ts";
import IconCheck from "~icons/lucide/check";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronUp from "~icons/lucide/chevron-up";
import { createControllable, toBoolean, type MouseHandler } from "../controllable.ts";
import { composeRef, useAnchoredPanel, type ElementRef } from "../position.ts";
import { ANCHORED_PANEL_ATTRIBUTE, dismissOnOutsidePointer } from "../position.ts";
import { Separator as SeparatorRoot } from "../separator/index.ts";

/**
 * Select, ported from `ui/src/lib/components/ui/select`.
 *
 * Composition: Terracotta's `Listbox` is the whole "button + popup list"
 * machine (open state and selected value in one root), its `ListboxOptions` is
 * the popup `ul`, and `ListboxOption` is one `li`. `Select` is therefore a thin
 * naming layer: popup positioning comes from `useAnchoredPanel`, and the
 * selected label is tracked here because Terracotta stores values only.
 *
 * `Value` resolves the label from the items that have registered themselves.
 * Registration is not undone when a popup closes, so the trigger keeps showing
 * a label while the list is unmounted.
 */

interface SelectState {
  value: () => unknown;
  setValue: (value: unknown) => void;
  open: () => boolean;
  setOpen: (open: boolean) => void;
  anchor: () => HTMLElement | undefined;
  setAnchor: (element: HTMLElement) => void;
  labelFor: (value: unknown) => string | undefined;
  registerLabel: (value: unknown, label: () => string | undefined) => void;
}

const SelectContext = createContext<SelectState | null>(null);

function useSelect(component: string): SelectState {
  const state = useContext(SelectContext);
  if (!state) throw new Error(`<${component}> must be used inside a <Select.Root>`);
  return state;
}

interface PanelState {
  element: () => HTMLElement | undefined;
  setElement: (element: HTMLElement) => void;
  canScrollUp: () => boolean;
  canScrollDown: () => boolean;
  refresh: () => void;
  scrollUp: () => void;
  scrollDown: () => void;
}

const PanelContext = createContext<PanelState | null>(null);

function usePanel(component: string): PanelState {
  const state = useContext(PanelContext);
  if (!state) throw new Error(`<${component}> must be used inside a <Select.Content>`);
  return state;
}

export interface RootProps<V> {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  value?: V;
  defaultValue?: V;
  onValueChange?: (value: V | undefined) => void;
  disabled?: boolean;
  /** bits-ui's `type`; only `single` is supported. */
  type?: "single" | "multiple";
  children?: JSX.Element;
}

export function Root<V>(props: RootProps<V>) {
  const [open, setOpen] = createControllable(
    () => props.open,
    props.onOpenChange,
    props.defaultOpen ?? false,
  );
  const [value, setValue] = createControllable<V | undefined>(
    () => props.value,
    props.onValueChange,
    props.defaultValue,
  );
  const [anchor, setAnchorElement] = createSignal<HTMLElement>();
  const [labels, setLabels] = createSignal(new Map<unknown, () => string | undefined>());

  const state: SelectState = {
    value,
    setValue: (next) => setValue(next as V | undefined),
    open,
    setOpen,
    anchor,
    setAnchor: (element) => setAnchorElement(() => element),
    labelFor: (candidate) => labels().get(candidate)?.(),
    registerLabel: (candidate, label) => {
      setLabels((previous) => {
        const next = new Map(previous);
        next.set(candidate, label);
        return next;
      });
    },
  };

  return (
    <SelectContext value={state}>
      <Listbox
        isOpen={open()}
        onDisclosureChange={(next) => setOpen(next)}
        value={value()}
        onSelectChange={(next) => setValue(next)}
        disabled={props.disabled}
      >
        {props.children}
      </Listbox>
    </SelectContext>
  );
}

export interface TriggerProps extends Omit<ComponentProps<"button">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  size?: "sm" | "default";
}

export function Trigger(props: TriggerProps) {
  const state = useSelect("Select.Trigger");
  const rest = omit(props, "class", "children", "ref", "onClick", "disabled", "size");

  return (
    <ListboxButton
      {...rest}
      ref={composeRef(props.ref, (element: HTMLButtonElement) => state.setAnchor(element))}
      data-slot="select-trigger"
      data-size={props.size ?? "default"}
      disabled={toBoolean(props.disabled)}
      class={cn(
        "border-input data-[placeholder]:text-muted-foreground dark:bg-input/30 dark:hover:bg-input/50 focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 gap-1.5 rounded-md border bg-transparent py-2 pr-2 pl-2.5 text-sm shadow-xs transition-[color,box-shadow] focus-visible:ring-3 aria-invalid:ring-3 data-[size=default]:h-9 data-[size=sm]:h-8 *:data-[slot=select-value]:flex *:data-[slot=select-value]:gap-1.5 [&_svg:not([class*='size-'])]:size-4 flex w-fit items-center justify-between whitespace-nowrap outline-none disabled:cursor-not-allowed disabled:opacity-50 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center [&_svg]:pointer-events-none [&_svg]:shrink-0",
        props.class,
      )}
      onClick={props.onClick}
    >
      {props.children}
      <IconChevronDown class="text-muted-foreground size-4 pointer-events-none" />
    </ListboxButton>
  );
}

export interface ValueProps extends ComponentProps<"span"> {
  placeholder?: string;
}

export function Value(props: ValueProps) {
  const state = useSelect("Select.Value");
  const rest = omit(props, "class", "children", "placeholder");

  const label = (): string | undefined => {
    const current = state.value();
    return current === undefined ? undefined : state.labelFor(current);
  };

  return (
    <span {...rest} data-slot="select-value" class={props.class}>
      {label() ?? props.placeholder ?? props.children}
    </span>
  );
}

export type PortalProps = {
  mount?: Element;
  children?: JSX.Element;
};

/** The panel is already portaled; this exists for API compatibility. */
export function PortalRoot(props: PortalProps) {
  return <Portal mount={props.mount}>{props.children}</Portal>;
}

export interface ContentProps extends Omit<ComponentProps<"ul">, "ref"> {
  ref?: ElementRef;
  side?: "top" | "right" | "bottom" | "left";
  align?: "start" | "center" | "end";
  sideOffset?: number;
}

export function Content(props: ContentProps) {
  const state = useSelect("Select.Content");
  const rest = omit(props, "class", "children", "ref", "side", "align", "sideOffset");

  dismissOnOutsidePointer({
    open: state.open,
    anchor: state.anchor,
    onDismiss: () => state.setOpen(false),
  });

  const panel = useAnchoredPanel({
    anchor: state.anchor,
    open: state.open,
    side: () => props.side ?? "bottom",
    align: () => props.align ?? "start",
    sideOffset: () => props.sideOffset ?? 4,
    matchWidth: true,
  });

  const [element, setElement] = createSignal<HTMLElement>();
  const [scrollTop, setScrollTop] = createSignal(0);
  const [metrics, setMetrics] = createSignal({ height: 0, visible: 0 });

  const measure = (target: HTMLElement) => {
    setScrollTop(target.scrollTop);
    setMetrics({ height: target.scrollHeight, visible: target.clientHeight });
  };

  createEffect(element, (current) => {
    if (!current) return;
    const refresh = () => measure(current);
    refresh();
    current.addEventListener("scroll", refresh, { passive: true });
    const observer = new ResizeObserver(refresh);
    observer.observe(current);
    return () => {
      current.removeEventListener("scroll", refresh);
      observer.disconnect();
    };
  });

  const canScrollUp = () => scrollTop() > 1;
  const canScrollDown = () => metrics().height - metrics().visible - scrollTop() > 1;
  const step = () => Math.max(32, Math.round(metrics().visible / 2));

  const panelState: PanelState = {
    element,
    setElement: (next) => setElement(() => next),
    canScrollUp,
    canScrollDown,
    refresh: () => {
      const current = element();
      if (current) measure(current);
    },
    scrollUp: () => element()?.scrollBy({ top: -step(), behavior: "smooth" }),
    scrollDown: () => element()?.scrollBy({ top: step(), behavior: "smooth" }),
  };

  return (
    <Portal>
      <ListboxOptions
        {...rest}
        // Kept mounted so every item's label registers even before the first
        // open: `Value` then shows the selected label, not the placeholder.
        unmount={false}
        ref={composeRef(panel.ref, (next: HTMLElement) => {
          panelState.setElement(next);
          props.ref?.(next);
        })}
        {...{ [ANCHORED_PANEL_ATTRIBUTE]: "" }}
        data-slot="select-content"
        class={cn(
          "bg-popover text-popover-foreground animate-in fade-in-0 zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 min-w-36 rounded-md shadow-md ring-1 duration-100 relative z-50 overflow-x-hidden overflow-y-auto [&:not([tc-expanded])]:hidden",
          props.class,
        )}
      >
        <PanelContext value={panelState}>
          <ScrollUpButton />
          {props.children}
          <ScrollDownButton />
        </PanelContext>
      </ListboxOptions>
    </Portal>
  );
}

export interface ItemProps extends Omit<ComponentProps<"li">, "ref" | "onClick" | "value"> {
  ref?: ElementRef<HTMLLIElement>;
  value: unknown;
  label?: string;
  disabled?: boolean;
}

export function Item(props: ItemProps) {
  const state = useSelect("Select.Item");
  const label = () => props.label ?? String(props.value);

  onSettled(() => {
    state.registerLabel(props.value, label);
  });

  const rest = omit(props, "class", "children", "ref", "value", "label", "disabled");
  const selected = () => state.value() === props.value;

  return (
    <ListboxOption
      {...rest}
      ref={props.ref}
      value={props.value}
      disabled={props.disabled}
      data-slot="select-item"
      class={cn(
        "focus:bg-accent focus:text-accent-foreground hover:bg-accent hover:text-accent-foreground not-data-[variant=destructive]:focus:**:text-accent-foreground gap-2 rounded-sm py-1.5 pr-8 pl-2 text-sm [&_svg:not([class*='size-'])]:size-4 *:[span]:last:flex *:[span]:last:items-center *:[span]:last:gap-2 relative flex w-full cursor-default items-center outline-hidden select-none [&[tc-disabled]]:pointer-events-none [&[tc-disabled]]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
        props.class,
      )}
    >
      <span class="pointer-events-none absolute right-2 flex size-4 items-center justify-center">
        {selected() ? <IconCheck class="pointer-events-none" /> : null}
      </span>
      <span class="flex flex-1 gap-2 shrink-0 whitespace-nowrap">{props.children ?? label()}</span>
    </ListboxOption>
  );
}

export type GroupProps = ComponentProps<"div">;

export function Group(props: GroupProps) {
  const rest = omit(props, "class", "children");
  return (
    <div {...rest} data-slot="select-group" role="group" class={cn("scroll-my-1 p-1", props.class)}>
      {props.children}
    </div>
  );
}

export type GroupHeadingProps = ComponentProps<"div">;

export function GroupHeading(props: GroupHeadingProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="select-group-heading"
      class={cn("text-muted-foreground px-2 py-1.5 text-xs", props.class)}
    >
      {props.children}
    </div>
  );
}

export type LabelProps = ComponentProps<"div">;

export function Label(props: LabelProps) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="select-label"
      class={cn("text-muted-foreground px-2 py-1.5 text-xs", props.class)}
    >
      {props.children}
    </div>
  );
}

export type SeparatorProps = ComponentProps<"div">;

export function Separator(props: SeparatorProps) {
  const rest = omit(props, "class");
  return (
    <SeparatorRoot
      {...rest}
      data-slot="select-separator"
      class={cn("bg-border -mx-1 my-1 h-px pointer-events-none", props.class)}
    />
  );
}

export interface ScrollButtonProps extends Omit<ComponentProps<"button">, "ref"> {
  ref?: ElementRef<HTMLButtonElement>;
}

function ScrollButton(props: ScrollButtonProps & { direction: "up" | "down" }) {
  const panel = usePanel("Select.ScrollButton");
  const visible = () => (props.direction === "up" ? panel.canScrollUp() : panel.canScrollDown());
  const rest = omit(props, "class", "children", "ref", "direction");

  return (
    <button
      {...rest}
      ref={(element) => props.ref?.(element)}
      data-slot={props.direction === "up" ? "select-scroll-up-button" : "select-scroll-down-button"}
      type="button"
      tabindex={-1}
      class={cn(
        "bg-popover z-10 flex cursor-default items-center justify-center py-1 [&_svg:not([class*='size-'])]:size-4",
        visible() ? undefined : "hidden",
        props.class,
      )}
      onClick={() => {
        if (props.direction === "up") panel.scrollUp();
        else panel.scrollDown();
      }}
    >
      {props.children ?? (props.direction === "up" ? <IconChevronUp /> : <IconChevronDown />)}
    </button>
  );
}

export function ScrollUpButton(props: ScrollButtonProps) {
  return <ScrollButton {...props} direction="up" />;
}

export function ScrollDownButton(props: ScrollButtonProps) {
  return <ScrollButton {...props} direction="down" />;
}
