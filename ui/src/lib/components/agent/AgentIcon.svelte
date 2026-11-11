<script lang="ts">
  import BotIcon from "@lucide/svelte/icons/bot";
  import { cn } from "#lib/utils.ts";

  let {
    icon = null,
    class: className = "size-4",
  }: {
    /** `AgentSummary.icon` (a built-in asset key or a URL). */
    icon?: string | null;
    class?: string;
  } = $props();

  /** Built-in agent icons bundled with the Web UI. */
  const BUILTIN_ICONS: Readonly<Record<string, string>> = {
    "builtin:opencode": "/agents/opencode.png",
  };

  const src = $derived(resolveIcon(icon));

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
</script>

{#if src !== null}
  <img
    {src}
    alt=""
    aria-hidden="true"
    loading="lazy"
    class={cn("shrink-0 object-contain", className)}
  />
{:else}
  <BotIcon aria-hidden="true" class={cn("text-muted-foreground shrink-0", className)} />
{/if}
