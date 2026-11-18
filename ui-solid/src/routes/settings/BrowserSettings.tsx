import { useNavigate, useParams } from "@solidjs/router";
import { createEffect, createMemo, For, Match, Show, Switch } from "solid-js";
import NotificationsPanel from "#lib/components/settings/NotificationsPanel.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import * as Select from "#lib/components/ui/select/index.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  browserSettingsPath,
  isBrowserTab,
  resolveDefaultNodeId,
  settingsHomePath,
  type BrowserSettingsTab,
} from "#lib/settings-routes.ts";
import { sync } from "#lib/stores/app.ts";
import { pinned } from "#lib/stores/pinned.ts";
import { prefs } from "#lib/stores/prefs.ts";
import { setMode, userPrefersMode } from "#lib/mode.ts";
import { PALETTES, theme, type PaletteId } from "#lib/theme.ts";
import { cn } from "#lib/utils.ts";

/** `/settings/browser/:tab?`: per-browser (this device) settings. */
export default function BrowserSettings() {
  const params = useParams<{ tab?: string }>();
  const navigate = useNavigate();

  const isHub = () => sync.connection.role === "hub";
  const defaultNodeId = createMemo(() =>
    resolveDefaultNodeId(isHub(), sync.nodes, sync.connection.systemInfo?.node_id),
  );
  const vapidSubjectHref = createMemo(() => settingsHomePath(isHub(), defaultNodeId()));

  const tab = createMemo<BrowserSettingsTab>(() => {
    const raw = params.tab;
    return raw !== undefined && isBrowserTab(raw) ? raw : "appearance";
  });

  createEffect(
    () => params.tab ?? null,
    (raw) => {
      if (raw === null) navigate(browserSettingsPath("appearance"), { replace: true });
    },
  );

  /** Theme preference stored in this browser (`system` / `light` / `dark`). */
  const themePreference = () => userPrefersMode();

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

  return (
    <Show when={params.tab !== undefined}>
      <Switch>
        <Match when={tab() === "appearance"}>
          <Card class="gap-4 p-5">
            <div class="flex flex-col gap-1">
              <h2 class="text-sm font-semibold">{m.settings_appearance_theme()}</h2>
              <p class="text-muted-foreground text-[13px]">{m.settings_appearance_theme_note()}</p>
            </div>
            <div class="grid max-w-sm gap-1.5">
              <Select.Root
                type="single"
                value={themePreference() ?? "system"}
                onValueChange={(value) => setMode(value as "system" | "light" | "dark")}
              >
                <Select.Trigger
                  id="browser-theme"
                  size="sm"
                  class="w-full text-[13px]"
                  aria-label={m.settings_appearance_theme()}
                >
                  <span>{themeModeLabel(themePreference() ?? "system")}</span>
                </Select.Trigger>
                <Select.Content>
                  <Select.Item
                    value="system"
                    label={m.settings_browser_theme_system()}
                    class="text-[13px]"
                  >
                    {m.settings_browser_theme_system()}
                  </Select.Item>
                  <Select.Item
                    value="light"
                    label={m.settings_browser_theme_light()}
                    class="text-[13px]"
                  >
                    {m.settings_browser_theme_light()}
                  </Select.Item>
                  <Select.Item
                    value="dark"
                    label={m.settings_browser_theme_dark()}
                    class="text-[13px]"
                  >
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
              <For each={PALETTES}>
                {(option) => (
                  <button
                    type="button"
                    aria-pressed={theme.palette === option.id ? "true" : "false"}
                    onClick={() => theme.setPalette(option.id)}
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
                        style={{ background: option.canvas }}
                      ></span>
                      <span
                        class="size-3.5 rounded-full border border-black/10"
                        style={{ background: option.ink }}
                      ></span>
                      <span
                        class="size-3.5 rounded-full border border-black/10"
                        style={{ background: option.signal }}
                      ></span>
                    </span>
                    <span class="text-[13px] font-medium">{paletteLabel(option.id)}</span>
                  </button>
                )}
              </For>
            </div>
            <p class="text-muted-foreground text-[13px]">{m.settings_appearance_storage_note()}</p>
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
                onValueChange={(value) =>
                  prefs.setSendShortcut(value as "enter" | "modifier_enter")
                }
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
            <p class="text-muted-foreground text-[13px]">{m.settings_browser_pinned_note()}</p>
            <div class="flex items-center justify-between gap-3">
              <span class="text-muted-foreground text-[13px]">
                {m.settings_browser_pinned_count({ count: pinned.pinnedIds.size })}
              </span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={pinned.pinnedIds.size === 0}
                onClick={() => pinned.clear()}
              >
                {m.settings_browser_unpin_all()}
              </Button>
            </div>
          </Card>
        </Match>

        <Match when={tab() === "notifications"}>
          <NotificationsPanel vapidSubjectHref={vapidSubjectHref()} />
        </Match>
      </Switch>
    </Show>
  );
}
