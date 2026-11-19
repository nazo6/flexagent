import type { ComponentProps, JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import { createContext, createSignal, omit, Show, useContext } from "solid-js";
import { Popover, PopoverButton, PopoverPanel } from "terracotta/popover";
import { Menu, MenuItem } from "terracotta/menu";
import { cn } from "#lib/utils.ts";
import { createControllable, toBoolean, type MouseHandler } from "../controllable.ts";
import {
  composeRef,
  createPanelTree,
  PanelTreeContext,
  registerPanelElement,
  useAnchoredPanel,
  type ElementRef,
  type PanelAlign,
} from "../position.ts";
import { ANCHORED_PANEL_ATTRIBUTE, dismissOnOutsidePointer } from "../position.ts";
import IconCheck from "~icons/lucide/check";
import IconMinus from "~icons/lucide/minus";
import IconChevronRight from "~icons/lucide/chevron-right";
import { Separator } from "../separator/index.ts";

/**
 * Dropdown menu, ported from `ui/src/lib/components/ui/dropdown-menu`.
 *
 * Composition: Terracotta splits the "open, anchored popup" (`Popover`) from
 * the "arrow-key navigable list of actions" (`Menu`). `Content` is
 * `Portal + PopoverPanel + Menu`; `Sub` is a nested `Popover` whose panel is a
 * second `Menu`, which Terracotta treats as part of the outer panel for focus
 * and click-outside purposes.
 *
 * Native `class` and the rest of the props are forwarded; `onSelect` is called
 * after the item's click, matching bits-ui's name.
 */

interface MenuState {
  open: () => boolean;
  setOpen: (open: boolean) => void;
  anchor: () => HTMLElement | undefined;
  setAnchor: (element: HTMLElement) => void;
  /** Closes this menu and every menu it is nested in. */
  closeMenu: () => void;
}

const RootContext = createContext<MenuState | null>(null);
const SubContext = createContext<MenuState | null>(null);

function useMenu(component: string): MenuState {
  const state = useContext(RootContext);
  if (!state) throw new Error(`<${component}> must be used inside a <DropdownMenu.Root>`);
  return state;
}

function useSub(component: string): MenuState {
  const state = useContext(SubContext);
  if (!state) throw new Error(`<${component}> must be used inside a <DropdownMenu.Sub>`);
  return state;
}

interface CheckboxGroupState {
  value: () => unknown[] | undefined;
  toggle: (value: unknown, next: boolean) => void;
}

interface RadioGroupState {
  value: () => unknown;
  select: (value: unknown) => void;
}

const CheckboxGroupContext = createContext<CheckboxGroupState | null>(null);
const RadioGroupContext = createContext<RadioGroupState | null>(null);

export interface RootProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  children?: JSX.Element;
}

export function Root(props: RootProps) {
  const [open, setOpen] = createControllable(
    () => props.open,
    () => props.onOpenChange,
    () => props.defaultOpen ?? false,
  );
  const [anchor, setAnchorElement] = createSignal<HTMLElement>();

  const state: MenuState = {
    open,
    setOpen,
    anchor,
    setAnchor: (el) => setAnchorElement(() => el),
    closeMenu: () => setOpen(false),
  };

  return (
    <RootContext value={state}>
      <Popover isOpen={open()} onChange={setOpen}>
        {props.children}
      </Popover>
    </RootContext>
  );
}

export interface TriggerProps extends Omit<ComponentProps<"button">, "ref" | "onClick" | "type"> {
  ref?: ElementRef<HTMLButtonElement>;
  onClick?: MouseHandler<HTMLButtonElement>;
  type?: "button" | "submit" | "reset";
}

export function Trigger(props: TriggerProps) {
  const state = useMenu("DropdownMenu.Trigger");
  const rest = omit(props, "class", "children", "ref", "onClick", "disabled", "type");

  return (
    <PopoverButton
      {...rest}
      ref={composeRef(props.ref, (element: HTMLButtonElement) => state.setAnchor(element))}
      data-slot="dropdown-menu-trigger"
      aria-haspopup="menu"
      type={props.type ?? "button"}
      disabled={props.disabled === undefined ? undefined : toBoolean(props.disabled)}
      class={props.class}
      onClick={props.onClick}
    >
      {props.children}
    </PopoverButton>
  );
}

export type PortalProps = {
  mount?: Element;
  children?: JSX.Element;
};

/** The panels are already portaled; this exists for API compatibility. */
export function PortalRoot(props: PortalProps) {
  return <Portal mount={props.mount}>{props.children}</Portal>;
}

const CONTENT_CLASSES =
  "animate-in fade-in-0 zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-32 rounded-md p-1 shadow-md ring-1 duration-100 z-50 overflow-x-hidden overflow-y-auto";

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  side?: "top" | "right" | "bottom" | "left";
  align?: PanelAlign;
  sideOffset?: number;
  alignOffset?: number;
}

export function Content(props: ContentProps) {
  const state = useMenu("DropdownMenu.Content");
  const tree = createPanelTree();
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "side",
    "align",
    "sideOffset",
    "alignOffset",
  );

  dismissOnOutsidePointer({
    open: state.open,
    anchor: state.anchor,
    tree,
    onDismiss: () => state.closeMenu(),
  });

  const panel = useAnchoredPanel({
    anchor: state.anchor,
    open: state.open,
    side: () => props.side ?? "bottom",
    align: () => props.align ?? "start",
    sideOffset: () => props.sideOffset ?? 4,
    alignOffset: () => props.alignOffset ?? 0,
  });

  return (
    <Show when={state.open()}>
      <Portal>
        <PopoverPanel
          {...rest}
          ref={composeRef(panel.ref, (element: HTMLElement) => {
            registerPanelElement(tree, element);
            props.ref?.(element);
          })}
          {...{ [ANCHORED_PANEL_ATTRIBUTE]: "" }}
          data-slot="dropdown-menu-content"
          class={cn(CONTENT_CLASSES, props.class)}
        >
          <PanelTreeContext value={tree}>
            <Menu>{props.children}</Menu>
          </PanelTreeContext>
        </PopoverPanel>
      </Portal>
    </Show>
  );
}

export interface ItemProps extends Omit<ComponentProps<"li">, "ref" | "onClick" | "onSelect"> {
  ref?: ElementRef<HTMLLIElement>;
  inset?: boolean;
  variant?: "default" | "destructive";
  disabled?: boolean;
  /** Called when the item is activated; the menu closes afterwards. */
  onSelect?: (event: MouseEvent) => void;
  onClick?: MouseHandler<HTMLLIElement>;
}

const ITEM_CLASSES =
  "focus:bg-accent focus:text-accent-foreground hover:bg-accent hover:text-accent-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 dark:data-[variant=destructive]:focus:bg-destructive/20 data-[variant=destructive]:focus:text-destructive data-[variant=destructive]:*:[svg]:text-destructive not-data-[variant=destructive]:focus:**:text-accent-foreground gap-2 rounded-sm px-2 py-1.5 text-sm data-[inset]:pl-8 [&_svg:not([class*='size-'])]:size-4 group/dropdown-menu-item relative flex cursor-default items-center outline-hidden select-none [&[tc-disabled]]:pointer-events-none [&[tc-disabled]]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0";

export function Item(props: ItemProps) {
  const sub = useContext(SubContext);
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "inset",
    "variant",
    "disabled",
    "onSelect",
    "onClick",
  );

  const handleClick: MouseHandler<HTMLLIElement> = (event) => {
    props.onClick?.(event);
    if (props.disabled) return;
    props.onSelect?.(event);
    if (event.defaultPrevented) {
      // The caller keeps the menu open: stop the click before the panel's own
      // menu-item activation closes it.
      event.stopPropagation();
      return;
    }
    // The outer panel is not an ancestor of a submenu's portal, so a selection
    // there must close the submenu and its parent explicitly. A root item's
    // panel closes itself; calling it here would emit a second close.
    sub?.closeMenu();
  };

  // A native listener runs at the item itself, before the panel's bubble-phase
  // activation listener; a delegated `onClick` would run too late to stop it.
  const setRef = (element: HTMLLIElement) => {
    props.ref?.(element);
    element.addEventListener("click", handleClick as (event: Event) => void);
  };

  return (
    <MenuItem
      {...rest}
      ref={setRef}
      data-slot="dropdown-menu-item"
      data-inset={props.inset}
      data-variant={props.variant ?? "default"}
      disabled={props.disabled}
      class={cn(ITEM_CLASSES, props.class)}
    >
      {props.children}
    </MenuItem>
  );
}

export interface CheckboxGroupProps extends Omit<ComponentProps<"div">, "onChange"> {
  value?: unknown[];
  onValueChange?: (value: unknown[]) => void;
}

export function CheckboxGroup(props: CheckboxGroupProps) {
  const rest = omit(props, "class", "children", "value", "onValueChange");
  const [selected, setSelected] = createControllable<unknown[]>(
    () => props.value,
    () => props.onValueChange,
    () => [],
  );
  return (
    <CheckboxGroupContext
      value={{
        value: selected,
        toggle: (value, next) => {
          const current = selected();
          const nextValue = next
            ? [...current.filter((entry) => entry !== value), value]
            : current.filter((entry) => entry !== value);
          setSelected(nextValue);
        },
      }}
    >
      <div {...rest} data-slot="dropdown-menu-checkbox-group" role="group" class={props.class}>
        {props.children}
      </div>
    </CheckboxGroupContext>
  );
}

export interface CheckboxItemProps extends Omit<ComponentProps<"li">, "ref" | "onClick" | "value"> {
  ref?: ElementRef<HTMLLIElement>;
  checked?: boolean;
  indeterminate?: boolean;
  /** Groups own the checked set; pass `value` instead of `checked`. */
  value?: unknown;
  closeOnSelect?: boolean;
  disabled?: boolean;
  inset?: boolean;
  onCheckedChange?: (checked: boolean) => void;
  onClick?: MouseHandler<HTMLLIElement>;
}

export function CheckboxItem(props: CheckboxItemProps) {
  const group = useContext(CheckboxGroupContext);
  const menu = useContext(SubContext) ?? useContext(RootContext);

  const checked = (): boolean => {
    if (props.checked !== undefined) return props.checked;
    if (!group || props.value === undefined) return false;
    return group.value()?.includes(props.value) ?? false;
  };

  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "checked",
    "indeterminate",
    "value",
    "closeOnSelect",
    "disabled",
    "inset",
    "onCheckedChange",
    "onClick",
  );

  const handleClick: MouseHandler<HTMLLIElement> = (event) => {
    props.onClick?.(event);
    if (props.disabled) return;
    const next = !checked();
    if (props.checked === undefined && group && props.value !== undefined) {
      group.toggle(props.value, next);
    }
    props.onCheckedChange?.(next);
    if (props.closeOnSelect && !event.defaultPrevented) menu?.closeMenu();
  };

  return (
    <MenuItem
      {...rest}
      ref={props.ref}
      data-slot="dropdown-menu-checkbox-item"
      data-inset={props.inset}
      role="menuitemcheckbox"
      aria-checked={props.indeterminate ? "mixed" : checked() ? "true" : "false"}
      disabled={props.disabled}
      class={cn(ITEM_CLASSES, "pr-8", props.class)}
      onClick={handleClick}
    >
      <span
        class="absolute right-2 flex items-center justify-center pointer-events-none"
        data-slot="dropdown-menu-checkbox-item-indicator"
      >
        {props.indeterminate ? <IconMinus /> : checked() ? <IconCheck /> : null}
      </span>
      {props.children}
    </MenuItem>
  );
}

export interface RadioGroupProps extends Omit<ComponentProps<"div">, "onChange"> {
  value?: unknown;
  onValueChange?: (value: unknown) => void;
}

export function RadioGroup(props: RadioGroupProps) {
  const rest = omit(props, "class", "children", "value", "onValueChange");
  const [selected, setSelected] = createControllable<unknown>(
    () => props.value,
    () => props.onValueChange,
    () => undefined,
  );
  return (
    <RadioGroupContext
      value={{
        value: selected,
        select: (next) => setSelected(next),
      }}
    >
      <div {...rest} data-slot="dropdown-menu-radio-group" role="group" class={props.class}>
        {props.children}
      </div>
    </RadioGroupContext>
  );
}

export interface RadioItemProps extends Omit<ComponentProps<"li">, "ref" | "onClick" | "value"> {
  ref?: ElementRef<HTMLLIElement>;
  value: unknown;
  closeOnSelect?: boolean;
  disabled?: boolean;
  inset?: boolean;
  onCheckedChange?: (checked: boolean) => void;
  onClick?: MouseHandler<HTMLLIElement>;
}

export function RadioItem(props: RadioItemProps) {
  const group = useContext(RadioGroupContext);
  const menu = useContext(SubContext) ?? useContext(RootContext);
  const checked = () => group?.value() === props.value;

  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "value",
    "closeOnSelect",
    "disabled",
    "inset",
    "onCheckedChange",
    "onClick",
  );

  const handleClick: MouseHandler<HTMLLIElement> = (event) => {
    props.onClick?.(event);
    if (props.disabled) return;
    group?.select(props.value);
    props.onCheckedChange?.(true);
    if (props.closeOnSelect && !event.defaultPrevented) menu?.closeMenu();
  };

  return (
    <MenuItem
      {...rest}
      ref={props.ref}
      data-slot="dropdown-menu-radio-item"
      data-inset={props.inset}
      role="menuitemradio"
      aria-checked={checked() ? "true" : "false"}
      disabled={props.disabled}
      class={cn(ITEM_CLASSES, "pr-8", props.class)}
      onClick={handleClick}
    >
      <span
        class="absolute right-2 flex items-center justify-center pointer-events-none"
        data-slot="dropdown-menu-radio-item-indicator"
      >
        {checked() ? <IconCheck /> : null}
      </span>
      {props.children}
    </MenuItem>
  );
}

export type GroupProps = ComponentProps<"div">;

export function Group(props: GroupProps) {
  const rest = omit(props, "class", "children");
  return (
    <div {...rest} data-slot="dropdown-menu-group" role="group" class={props.class}>
      {props.children}
    </div>
  );
}

export type GroupHeadingProps = ComponentProps<"div"> & { inset?: boolean };

export function GroupHeading(props: GroupHeadingProps) {
  const rest = omit(props, "class", "children", "inset");
  return (
    <div
      {...rest}
      data-slot="dropdown-menu-group-heading"
      data-inset={props.inset}
      class={cn(
        "text-muted-foreground px-2 py-1.5 text-xs font-medium data-[inset]:pl-8",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export type LabelProps = ComponentProps<"div"> & { inset?: boolean };

export function Label(props: LabelProps) {
  const rest = omit(props, "class", "children", "inset");
  return (
    <div
      {...rest}
      data-slot="dropdown-menu-label"
      data-inset={props.inset}
      class={cn(
        "text-muted-foreground px-2 py-1.5 text-xs font-medium data-[inset]:pl-8",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export type SeparatorProps = ComponentProps<"div">;

export function SeparatorRoot(props: SeparatorProps) {
  const rest = omit(props, "class");
  return (
    <Separator
      {...rest}
      data-slot="dropdown-menu-separator"
      class={cn("bg-border -mx-1 my-1 h-px", props.class)}
    />
  );
}

export type ShortcutProps = ComponentProps<"span">;

export function Shortcut(props: ShortcutProps) {
  const rest = omit(props, "class", "children");
  return (
    <span
      {...rest}
      data-slot="dropdown-menu-shortcut"
      class={cn(
        "text-muted-foreground group-focus/dropdown-menu-item:text-accent-foreground ml-auto text-xs tracking-widest",
        props.class,
      )}
    >
      {props.children}
    </span>
  );
}

export interface SubProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  children?: JSX.Element;
}

export function Sub(props: SubProps) {
  const [open, setOpen] = createControllable(
    () => props.open,
    () => props.onOpenChange,
    () => props.defaultOpen ?? false,
  );
  const [anchor, setAnchorElement] = createSignal<HTMLElement>();
  const parent = useContext(SubContext) ?? useContext(RootContext);

  const state: MenuState = {
    open,
    setOpen,
    anchor,
    setAnchor: (el) => setAnchorElement(() => el),
    closeMenu: () => {
      setOpen(false);
      parent?.closeMenu();
    },
  };

  return (
    <SubContext value={state}>
      <Popover isOpen={open()} onChange={setOpen}>
        {props.children}
      </Popover>
    </SubContext>
  );
}

export interface SubTriggerProps extends Omit<ComponentProps<"li">, "ref" | "onClick"> {
  ref?: ElementRef<HTMLLIElement>;
  inset?: boolean;
  disabled?: boolean;
  onClick?: MouseHandler<HTMLLIElement>;
}

export function SubTrigger(props: SubTriggerProps) {
  const sub = useSub("DropdownMenu.SubTrigger");
  const rest = omit(props, "class", "children", "ref", "inset", "disabled", "onClick");

  const toggle = () => sub.setOpen(!sub.open());

  const handleClick: MouseHandler<HTMLLIElement> = (event) => {
    props.onClick?.(event);
    if (event.defaultPrevented || props.disabled) return;
    // The trigger owns this click; the outer panel must not treat it as an
    // activation of its own menu item.
    event.stopPropagation();
    toggle();
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    if (props.disabled) return;
    if (event.key === "ArrowRight" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      event.stopPropagation();
      sub.setOpen(true);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      event.stopPropagation();
      sub.setOpen(false);
    }
  };

  const handleKeyUp = (event: KeyboardEvent) => {
    // Space would otherwise synthesize a click after keydown already opened the
    // submenu, toggling it straight back closed.
    if (event.key === " ") {
      event.preventDefault();
      event.stopPropagation();
    }
  };

  return (
    <MenuItem
      {...rest}
      ref={composeRef(props.ref, (element: HTMLLIElement) => sub.setAnchor(element))}
      data-slot="dropdown-menu-sub-trigger"
      data-inset={props.inset}
      aria-haspopup="menu"
      aria-expanded={sub.open() ? "true" : "false"}
      disabled={props.disabled}
      class={cn(
        "focus:bg-accent focus:text-accent-foreground hover:bg-accent hover:text-accent-foreground [&[aria-expanded=true]]:bg-accent [&[aria-expanded=true]]:text-accent-foreground not-data-[variant=destructive]:focus:**:text-accent-foreground gap-2 rounded-sm px-2 py-1.5 text-sm data-[inset]:pl-8 [&_svg:not([class*='size-'])]:size-4 flex cursor-default items-center outline-hidden select-none [&[tc-disabled]]:pointer-events-none [&[tc-disabled]]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
        props.class,
      )}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
      onKeyUp={handleKeyUp}
    >
      {props.children}
      <IconChevronRight class="ml-auto size-4" />
    </MenuItem>
  );
}

export interface SubContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  side?: "top" | "right" | "bottom" | "left";
  align?: PanelAlign;
  sideOffset?: number;
  alignOffset?: number;
}

export function SubContent(props: SubContentProps) {
  const sub = useSub("DropdownMenu.SubContent");
  const tree = createPanelTree();
  const rest = omit(
    props,
    "class",
    "children",
    "ref",
    "side",
    "align",
    "sideOffset",
    "alignOffset",
  );

  dismissOnOutsidePointer({
    open: sub.open,
    anchor: sub.anchor,
    tree,
    onDismiss: () => sub.closeMenu(),
  });

  const panel = useAnchoredPanel({
    anchor: sub.anchor,
    open: sub.open,
    side: () => props.side ?? "right",
    align: () => props.align ?? "start",
    sideOffset: () => props.sideOffset ?? 0,
    alignOffset: () => props.alignOffset ?? -4,
  });

  return (
    <Portal>
      <Show when={sub.open()}>
        <PopoverPanel
          {...rest}
          ref={composeRef(panel.ref, (element: HTMLElement) => {
            registerPanelElement(tree, element);
            props.ref?.(element);
          })}
          {...{ [ANCHORED_PANEL_ATTRIBUTE]: "" }}
          data-slot="dropdown-menu-sub-content"
          class={cn(
            "animate-in fade-in-0 zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-[96px] rounded-md p-1 shadow-lg ring-1 duration-100 z-50 overflow-hidden",
            props.class,
          )}
          onKeyDown={(event) => {
            if (event.key !== "ArrowLeft" || event.defaultPrevented) return;
            event.preventDefault();
            event.stopPropagation();
            sub.setOpen(false);
            sub.anchor()?.focus();
          }}
        >
          <PanelTreeContext value={tree}>
            <Menu>{props.children}</Menu>
          </PanelTreeContext>
        </PopoverPanel>
      </Show>
    </Portal>
  );
}
