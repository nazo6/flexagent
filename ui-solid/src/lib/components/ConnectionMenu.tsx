import { createMemo, createSignal, For, Show } from "solid-js";
import ConnectionScopeBadge from "#lib/components/ConnectionScopeBadge.tsx";
import { Button, buttonVariants } from "#lib/components/ui/button/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { formatRelativeTime } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import { connection, logoutAndStop, sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import IconCable from "~icons/lucide/cable";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconClock from "~icons/lucide/clock";
import IconLogOut from "~icons/lucide/log-out";
import IconPlus from "~icons/lucide/plus";
import IconServer from "~icons/lucide/server";

/** Log out (stop syncing + invalidate the Cookie) and report the outcome. */
async function logout(): Promise<void> {
  try {
    await logoutAndStop();
    toast(m.connection_logged_out());
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
}

/**
 * Connection target selector, ported from
 * `ui/src/lib/components/ConnectionMenu.svelte`.
 *
 * The trigger has three shapes: the outlined button on the error screen, the
 * Hub/Node scope badge (sidebar footer) and its compact single-line variant
 * (mobile header). The "add target" form is a dialog.
 */

export interface ConnectionMenuProps {
  class?: string;
  align?: "start" | "center" | "end";
  /**
   * Trigger style. `button` is the plain outlined button (error screen),
   * `scope` renders the Hub/Node identity badge as the dropdown trigger and
   * `scope-compact` its single-line variant.
   */
  variant?: "button" | "scope" | "scope-compact";
}

export default function ConnectionMenu(props: ConnectionMenuProps) {
  const align = () => props.align ?? "start";
  const variant = () => props.variant ?? "button";

  const [menuOpen, setMenuOpen] = createSignal(false);
  const [addOpen, setAddOpen] = createSignal(false);
  const [newLabel, setNewLabel] = createSignal("");
  const [newUrl, setNewUrl] = createSignal("");

  const roleLabel = createMemo(() =>
    connection.role === "hub"
      ? m.connection_role_hub()
      : connection.role === "local_node"
        ? m.connection_role_local_node()
        : m.connection_role_disconnected(),
  );
  const originLabel = createMemo(() => connection.origin || m.connection_same_origin());
  const targetTooltip = createMemo(() =>
    m.connection_current_target_tooltip({ role: roleLabel(), origin: originLabel() }),
  );

  const addTarget = (event: SubmitEvent): void => {
    event.preventDefault();
    if (!newUrl().trim()) return;
    try {
      const parsed = new URL(newUrl().trim());
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        throw new Error(m.connection_url_scheme_error());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : m.connection_url_invalid());
      return;
    }
    if (!connection.addTarget(newLabel(), newUrl())) {
      toast.error(m.connection_target_exists());
      return;
    }
    setAddOpen(false);
    setNewLabel("");
    setNewUrl("");
    toast.success(m.connection_target_added());
  };

  return (
    <>
      <DropdownMenu.Root open={menuOpen()} onOpenChange={setMenuOpen}>
        {/*
          `DropdownMenu.Trigger` renders the `<button>` itself, so the outlined
          variant borrows the Button classes instead of nesting a second button
          inside it.
        */}
        <DropdownMenu.Trigger
          class={cn(
            variant() === "button"
              ? cn(buttonVariants({ variant: "outline", size: "sm" }), "gap-1.5 text-[13px]")
              : "group relative block w-full rounded-md text-left outline-none transition-opacity hover:opacity-85 focus-visible:ring-2 focus-visible:ring-ring/40",
            props.class,
          )}
          title={targetTooltip()}
          aria-label={variant() === "button" ? undefined : m.connection_current_target()}
        >
          <Show
            when={variant() === "button"}
            fallback={
              <>
                <ConnectionScopeBadge
                  compact={variant() === "scope-compact"}
                  class={variant() === "scope" ? "w-full pr-6" : "w-full"}
                />
                <Show when={variant() === "scope"}>
                  <IconChevronDown
                    class={cn(
                      "text-muted-foreground/70 pointer-events-none absolute top-1/2 right-2 size-3 -translate-y-1/2 transition-transform",
                      { "rotate-180": menuOpen() },
                    )}
                  />
                </Show>
              </>
            }
          >
            <div class="flex items-center gap-1.5 min-w-0">
              <IconServer class="size-3.5 shrink-0 text-muted-foreground" />
              <span class="truncate">{roleLabel()}</span>
            </div>
            <IconChevronDown class="size-3 text-muted-foreground/70 shrink-0 ml-auto" />
          </Show>
        </DropdownMenu.Trigger>

        <DropdownMenu.Content align={align()} class="w-80">
          <DropdownMenu.Label>{m.connection_current_target()}</DropdownMenu.Label>
          <DropdownMenu.Item disabled>
            <span class="truncate">{originLabel()}</span>
            <span class="text-muted-foreground ml-auto text-[13px]">{roleLabel()}</span>
          </DropdownMenu.Item>
          <Show when={sync.lastSyncedAt !== null}>
            <DropdownMenu.Item disabled>
              <IconClock />
              <span>
                {m.shell_synced_relative({ time: formatRelativeTime(sync.lastSyncedAt) })}
              </span>
            </DropdownMenu.Item>
          </Show>
          <DropdownMenu.Separator />
          <Show when={connection.availableTargets.length > 0}>
            <DropdownMenu.Label>{m.connection_targets_label()}</DropdownMenu.Label>
            <For each={connection.availableTargets}>
              {(target) => (
                <DropdownMenu.Item onSelect={() => connection.switchTo(target.id)}>
                  <IconCable />
                  <span class="flex-1 truncate">{target.label}</span>
                  <span class="text-muted-foreground text-[13px]">{target.url}</span>
                </DropdownMenu.Item>
              )}
            </For>
            <DropdownMenu.Separator />
          </Show>
          <DropdownMenu.Item onSelect={() => setAddOpen(true)}>
            <IconPlus />
            {m.connection_add_target_example()}
          </DropdownMenu.Item>
          <DropdownMenu.Separator />
          <DropdownMenu.Item onSelect={logout}>
            <IconLogOut />
            {m.connection_logout()}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>

      <Dialog.Root open={addOpen()} onOpenChange={setAddOpen}>
        <Dialog.Content class="sm:max-w-md">
          <Dialog.Header>
            <Dialog.Title>{m.connection_add_target_title()}</Dialog.Title>
            <Dialog.Description>{m.connection_add_target_description()}</Dialog.Description>
          </Dialog.Header>
          <form class="flex flex-col gap-4" onSubmit={addTarget}>
            <div class="grid gap-2">
              <Label for="target-label">{m.connection_display_name()}</Label>
              <Input
                id="target-label"
                value={newLabel()}
                onValueChange={setNewLabel}
                placeholder={m.connection_display_name_placeholder()}
              />
            </div>
            <div class="grid gap-2">
              <Label for="target-url">URL</Label>
              <Input
                id="target-url"
                value={newUrl()}
                onValueChange={setNewUrl}
                placeholder={m.connection_url_placeholder()}
              />
            </div>
            <Dialog.Footer>
              <Button type="submit" disabled={newUrl().trim() === ""}>
                {m.connection_add()}
              </Button>
            </Dialog.Footer>
          </form>
        </Dialog.Content>
      </Dialog.Root>
    </>
  );
}
