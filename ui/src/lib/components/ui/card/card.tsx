import type { ComponentProps } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "#lib/utils.ts";

/**
 * Card, ported from `ui/src/lib/components/ui/card`.
 *
 * All seven parts keep their `data-slot` attributes so screenshot checks and
 * tests can select them.
 */

export interface CardProps extends ComponentProps<"div"> {
  size?: "default" | "sm";
}

export function Root(props: CardProps) {
  const rest = omit(props, "class", "children", "size");
  return (
    <div
      {...rest}
      data-slot="card"
      data-size={props.size ?? "default"}
      class={cn(
        "border-border bg-card text-card-foreground gap-(--card-spacing) overflow-hidden rounded-lg border py-(--card-spacing) text-sm [--card-spacing:--spacing(6)] has-[>img:first-child]:pt-0 data-[size=sm]:[--card-spacing:--spacing(4)] *:[img:first-child]:rounded-t-lg *:[img:last-child]:rounded-b-lg group/card flex flex-col",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export function Header(props: ComponentProps<"div">) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="card-header"
      class={cn(
        "gap-1 rounded-t-xl px-(--card-spacing) [.border-b]:pb-(--card-spacing) group/card-header @container/card-header grid auto-rows-min items-start has-data-[slot=card-action]:grid-cols-[1fr_auto] has-data-[slot=card-description]:grid-rows-[auto_auto]",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export function Title(props: ComponentProps<"div">) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="card-title"
      class={cn(
        "text-base leading-normal font-medium group-data-[size=sm]/card:text-sm",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export function Description(props: ComponentProps<"p">) {
  const rest = omit(props, "class", "children");
  return (
    <p
      {...rest}
      data-slot="card-description"
      class={cn("text-muted-foreground text-sm", props.class)}
    >
      {props.children}
    </p>
  );
}

export function Content(props: ComponentProps<"div">) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="card-content"
      class={cn("flex flex-col gap-3 px-(--card-spacing)", props.class)}
    >
      {props.children}
    </div>
  );
}

export function Footer(props: ComponentProps<"div">) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="card-footer"
      class={cn(
        "rounded-b-xl px-(--card-spacing) [.border-t]:pt-(--card-spacing) flex items-center",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

export function Action(props: ComponentProps<"div">) {
  const rest = omit(props, "class", "children");
  return (
    <div
      {...rest}
      data-slot="card-action"
      class={cn("col-start-2 row-span-2 row-start-1 self-start justify-self-end", props.class)}
    >
      {props.children}
    </div>
  );
}

/**
 * `import { Card } from ".../card/index.ts"` stays a component (the Svelte
 * index exported `Root as Card`), and its parts hang off it so `<Card.Header>`
 * reads the same as `Card.Header` from a namespace import.
 */
export const Card = Object.assign(Root, {
  Root,
  Header,
  Title,
  Description,
  Content,
  Footer,
  Action,
});
