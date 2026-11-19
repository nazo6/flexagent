import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { Tab, TabGroup, TabList, TabPanel } from "terracotta/tabs";
import { cn } from "#lib/utils.ts";
import { createControllable, type MouseHandler } from "../controllable.ts";
import type { ElementRef } from "../position.ts";

/**
 * Tabs, ported from `ui/src/lib/components/ui/tabs`.
 *
 * `data-active` (bits-ui) becomes `tc-selected` (Terracotta). The shadcn
 * stylesheet defined `data-horizontal`/`data-vertical` custom variants; this
 * app does not import it, so the orientation variants are written against the
 * `data-orientation` attribute Terracotta's parts do not set — `Root` sets it.
 */

const listBase =
  "rounded-lg p-[3px] group-data-[orientation=horizontal]/tabs:h-9 data-[variant=line]:rounded-none group/tabs-list inline-flex w-fit items-center justify-center text-muted-foreground group-data-[orientation=vertical]/tabs:h-fit group-data-[orientation=vertical]/tabs:flex-col";

const listVariants = {
  default: "bg-muted",
  line: "gap-1 bg-transparent",
} as const;

export type TabsListVariant = keyof typeof listVariants;

export function tabsListVariants(options: { variant?: TabsListVariant } = {}): string {
  return `${listBase} ${listVariants[options.variant ?? "default"]}`;
}

const triggerClass =
  "gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium group-data-[variant=default]/tabs-list:[&[tc-selected]]:shadow-sm group-data-[variant=line]/tabs-list:[&[tc-selected]]:shadow-none [&_svg:not([class*='size-'])]:size-4 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 relative inline-flex h-[calc(100%_-_1px)] flex-1 items-center justify-center whitespace-nowrap text-foreground/60 transition-all group-data-[orientation=vertical]/tabs:w-full group-data-[orientation=vertical]/tabs:justify-start hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring [&[tc-disabled]]:pointer-events-none [&[tc-disabled]]:opacity-50 dark:text-muted-foreground dark:hover:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 group-data-[variant=line]/tabs-list:bg-transparent group-data-[variant=line]/tabs-list:[&[tc-selected]]:bg-transparent dark:group-data-[variant=line]/tabs-list:[&[tc-selected]]:border-transparent dark:group-data-[variant=line]/tabs-list:[&[tc-selected]]:bg-transparent [&[tc-selected]]:bg-background [&[tc-selected]]:text-foreground dark:[&[tc-selected]]:border-input dark:[&[tc-selected]]:bg-input/30 dark:[&[tc-selected]]:text-foreground after:absolute after:bg-signal after:opacity-0 after:transition-opacity group-data-[orientation=horizontal]/tabs:after:inset-x-0 group-data-[orientation=horizontal]/tabs:after:bottom-[-5px] group-data-[orientation=horizontal]/tabs:after:h-0.5 group-data-[orientation=vertical]/tabs:after:inset-y-0 group-data-[orientation=vertical]/tabs:after:-right-1 group-data-[orientation=vertical]/tabs:after:w-0.5 group-data-[variant=line]/tabs-list:[&[tc-selected]]:after:opacity-100";

export interface RootProps extends Omit<ComponentProps<"div">, "onChange" | "ref"> {
  ref?: ElementRef;
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string | undefined) => void;
  orientation?: "horizontal" | "vertical";
  disabled?: boolean;
}

export function Root(props: RootProps) {
  const rest = omit(
    props,
    "class",
    "children",
    "value",
    "defaultValue",
    "onValueChange",
    "orientation",
    "disabled",
    "ref",
  );
  const orientation = () => props.orientation ?? "horizontal";
  // `defaultValue` seeds the uncontrolled state; a later `value` switches the
  // group to controlled mode without remounting it.
  const [value, setValue] = createControllable(
    () => props.value,
    () => props.onValueChange,
    () => props.defaultValue ?? "",
  );

  return (
    <TabGroup
      {...rest}
      ref={props.ref}
      value={value()}
      onChange={(next) => setValue(next ?? "")}
      disabled={props.disabled}
      horizontal={orientation() === "horizontal"}
      data-slot="tabs"
      data-orientation={orientation()}
      class={cn("gap-2 group/tabs flex data-[orientation=horizontal]:flex-col", props.class)}
    >
      {props.children}
    </TabGroup>
  );
}

export interface ListProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  variant?: TabsListVariant;
}

export function List(props: ListProps) {
  const rest = omit(props, "class", "children", "ref", "variant");
  return (
    <TabList
      {...rest}
      ref={props.ref}
      data-slot="tabs-list"
      data-variant={props.variant ?? "default"}
      class={cn(tabsListVariants({ variant: props.variant }), props.class)}
    >
      {props.children}
    </TabList>
  );
}

export interface TriggerProps extends Omit<ComponentProps<"div">, "ref" | "onClick"> {
  ref?: ElementRef;
  value: string;
  disabled?: boolean;
  onClick?: MouseHandler<HTMLDivElement>;
}

export function Trigger(props: TriggerProps) {
  const rest = omit(props, "class", "children", "ref", "value", "disabled", "onClick");
  return (
    <Tab
      {...rest}
      ref={props.ref}
      value={props.value}
      disabled={props.disabled}
      data-slot="tabs-trigger"
      class={cn(triggerClass, props.class)}
      onClick={props.onClick}
    >
      {props.children}
    </Tab>
  );
}

export interface ContentProps extends Omit<ComponentProps<"div">, "ref"> {
  ref?: ElementRef;
  value: string;
  /** Keep the panel mounted while another tab is selected. */
  unmount?: boolean;
}

export function Content(props: ContentProps) {
  const rest = omit(props, "class", "children", "ref", "value", "unmount");
  return (
    <TabPanel
      {...rest}
      ref={props.ref}
      value={props.value}
      unmount={props.unmount}
      data-slot="tabs-content"
      class={cn("text-sm flex-1 outline-none [&:not([tc-selected])]:hidden", props.class)}
    >
      {props.children}
    </TabPanel>
  );
}
