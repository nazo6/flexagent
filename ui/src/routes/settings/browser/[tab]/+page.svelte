<script lang="ts">
  import NotificationsPanel from "#lib/components/settings/NotificationsPanel.svelte";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Select from "#lib/components/ui/select/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { resolveDefaultNodeId, settingsHomePath } from "#lib/settings-routes.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { pinned } from "#lib/stores/pinned.svelte.ts";
  import { prefs } from "#lib/stores/prefs.svelte.ts";
  import { PALETTES, theme, type PaletteId } from "#lib/theme.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { setMode, userPrefersMode } from "mode-watcher";
  import type { PageData } from "./$types";

  let { data }: { data: PageData } = $props();

  const isHub = $derived(sync.connection.role === "hub");
  const defaultNodeId = $derived(
    resolveDefaultNodeId(isHub, sync.nodes, sync.connection.systemInfo?.node_id),
  );
  const vapidSubjectHref = $derived(settingsHomePath(isHub, defaultNodeId));

  /** Theme preference stored in this browser (`system` / `light` / `dark`). */
  const themePreference = $derived(userPrefersMode.current);

  function themeModeLabel(value: string): string {
    switch (value) {
      case "light":
        return m.settings_browser_theme_light();
      case "dark":
        return m.settings_browser_theme_dark();
      default:
        return m.settings_browser_theme_system();
    }
  }

  function shortcutLabel(value: string): string {
    return value === "modifier_enter"
      ? m.settings_browser_send_shortcut_modifier()
      : m.settings_browser_send_shortcut_enter();
  }

  function paletteLabel(id: PaletteId): string {
    switch (id) {
      case "bone":
        return m.settings_appearance_palette_bone();
      case "graphite":
        return m.settings_appearance_palette_graphite();
      case "moss":
        return m.settings_appearance_palette_moss();
    }
  }
</script>

{#if data.tab === "appearance"}
  <Card class="gap-4 p-5">
    <div class="flex flex-col gap-1">
      <h2 class="text-sm font-semibold">{m.settings_appearance_theme()}</h2>
      <p class="text-muted-foreground text-[13px]">
        {m.settings_appearance_theme_note()}
      </p>
    </div>
    <div class="grid max-w-sm gap-1.5">
      <Select.Root
        type="single"
        value={themePreference ?? "system"}
        onValueChange={(value) => setMode(value as "system" | "light" | "dark")}
      >
        <Select.Trigger
          id="browser-theme"
          size="sm"
          class="w-full text-[13px]"
          aria-label={m.settings_appearance_theme()}
        >
          <span>{themeModeLabel(themePreference ?? "system")}</span>
        </Select.Trigger>
        <Select.Content>
          <Select.Item value="system" label={m.settings_browser_theme_system()} class="text-[13px]">
            {m.settings_browser_theme_system()}
          </Select.Item>
          <Select.Item value="light" label={m.settings_browser_theme_light()} class="text-[13px]">
            {m.settings_browser_theme_light()}
          </Select.Item>
          <Select.Item value="dark" label={m.settings_browser_theme_dark()} class="text-[13px]">
            {m.settings_browser_theme_dark()}
          </Select.Item>
        </Select.Content>
      </Select.Root>
    </div>
  </Card>

  <Card class="gap-4 p-5">
    <div class="flex flex-col gap-1">
      <h2 class="text-sm font-semibold">{m.settings_appearance_palette()}</h2>
      <p class="text-muted-foreground text-[13px]">
        {m.settings_appearance_palette_note()}
      </p>
    </div>
    <div class="grid gap-2 sm:grid-cols-3">
      {#each PALETTES as option (option.id)}
        <button
          type="button"
          aria-pressed={theme.palette === option.id}
          onclick={() => theme.setPalette(option.id)}
          class={cn(
            "flex items-center justify-between gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
            theme.palette === option.id
              ? "border-signal bg-signal-soft/60"
              : "border-border hover:border-line-strong hover:bg-muted/50",
          )}
        >
          <span class="flex items-center gap-2">
            <span
              class="size-3.5 rounded-full border border-black/10"
              style={`background:${option.canvas}`}
            ></span>
            <span
              class="size-3.5 rounded-full border border-black/10"
              style={`background:${option.ink}`}
            ></span>
            <span
              class="size-3.5 rounded-full border border-black/10"
              style={`background:${option.signal}`}
            ></span>
          </span>
          <span class="text-[13px] font-medium">{paletteLabel(option.id)}</span>
        </button>
      {/each}
    </div>
    <p class="text-muted-foreground text-[13px]">
      {m.settings_appearance_storage_note()}
    </p>
  </Card>

  <Card class="gap-4 p-5">
    <div class="flex flex-col gap-1">
      <h2 class="text-sm font-semibold">{m.settings_browser_send_shortcut()}</h2>
      <p class="text-muted-foreground text-[13px]">
        {m.settings_browser_send_shortcut_note()}
      </p>
    </div>
    <div class="grid max-w-sm gap-1.5">
      <Select.Root
        type="single"
        value={prefs.sendShortcut}
        onValueChange={(value) => prefs.setSendShortcut(value as "enter" | "modifier_enter")}
      >
        <Select.Trigger
          id="browser-send-shortcut"
          size="sm"
          class="w-full text-[13px]"
          aria-label={m.settings_browser_send_shortcut()}
        >
          <span>{shortcutLabel(prefs.sendShortcut)}</span>
        </Select.Trigger>
        <Select.Content>
          <Select.Item
            value="enter"
            label={m.settings_browser_send_shortcut_enter()}
            class="text-[13px]"
          >
            {m.settings_browser_send_shortcut_enter()}
          </Select.Item>
          <Select.Item
            value="modifier_enter"
            label={m.settings_browser_send_shortcut_modifier()}
            class="text-[13px]"
          >
            {m.settings_browser_send_shortcut_modifier()}
          </Select.Item>
        </Select.Content>
      </Select.Root>
    </div>
  </Card>

  <Card class="gap-3 p-4">
    <h2 class="text-sm font-semibold">{m.settings_browser_pinned()}</h2>
    <p class="text-muted-foreground text-[13px]">
      {m.settings_browser_pinned_note()}
    </p>
    <div class="flex items-center justify-between gap-3">
      <span class="text-muted-foreground text-[13px]">
        {m.settings_browser_pinned_count({ count: pinned.pinnedIds.size })}
      </span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pinned.pinnedIds.size === 0}
        onclick={() => pinned.clear()}
      >
        {m.settings_browser_unpin_all()}
      </Button>
    </div>
  </Card>
{:else if data.tab === "notifications"}
  <NotificationsPanel {vapidSubjectHref} />
{/if}
