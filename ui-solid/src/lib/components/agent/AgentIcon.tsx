import { cn } from "#lib/utils.ts";
import IconBot from "~icons/lucide/bot";
import { Show } from "solid-js";
import { AGENT_GLYPHS } from "./agent-glyphs.ts";

/** Built-in raster icons bundled with the Web UI. */
const BUILTIN_ICONS: Readonly<Record<string, string>> = {
  "builtin:opencode": "/agents/opencode.png",
};

function resolveIcon(value: string | null): string | null {
  if (value === null || value === "") return null;
  const builtin = BUILTIN_ICONS[value];
  if (builtin !== undefined) return builtin;
  if (
    value.startsWith("https://") ||
    value.startsWith("http://") ||
    value.startsWith("data:") ||
    value.startsWith("/")
  ) {
    return value;
  }
  return null;
}

/** Agent icon: inline glyph for built-ins, image for URLs, bot otherwise. */
export default function AgentIcon(props: { icon?: string | null; class?: string }) {
  const glyph = () => (props.icon ? (AGENT_GLYPHS[props.icon] ?? null) : null);
  const src = () => (glyph() !== null ? null : resolveIcon(props.icon ?? null));
  const className = () => props.class ?? "size-4";

  return (
    <Show
      when={glyph()}
      fallback={
        <Show
          when={src()}
          fallback={<IconBot aria-hidden="true" class={cn("text-muted-foreground shrink-0", className())} />}
        >
          {(value) => (
            <img
              src={value()}
              alt=""
              aria-hidden="true"
              loading="lazy"
              class={cn("shrink-0 object-contain", className())}
            />
          )}
        </Show>
      }
    >
      {(value) => (
        <svg
          viewBox={value().viewBox}
          fill={value().fill ?? "none"}
          aria-hidden="true"
          class={cn("shrink-0", className())}
          innerHTML={value().body}
        />
      )}
    </Show>
  );
}
