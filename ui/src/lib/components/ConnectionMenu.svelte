<script lang="ts">
  import { onMount } from "svelte";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import * as DropdownMenu from "#lib/components/ui/dropdown-menu/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { connection, logoutAndStop, sync } from "#lib/stores/app.svelte.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import {
    disablePushNotifications,
    enablePushNotifications,
    pushCapability,
    type PushCapability,
  } from "#lib/push.ts";
  import { toast } from "svelte-sonner";
  import BellIcon from "@lucide/svelte/icons/bell";
  import BellOffIcon from "@lucide/svelte/icons/bell-off";
  import CableIcon from "@lucide/svelte/icons/cable";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ClockIcon from "@lucide/svelte/icons/clock";
  import LogOutIcon from "@lucide/svelte/icons/log-out";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import ServerIcon from "@lucide/svelte/icons/server";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import ConnectionScopeBadge from "#lib/components/ConnectionScopeBadge.svelte";

  interface Props {
    class?: string;
    align?: "start" | "center" | "end";
    /**
     * Trigger style. `button` is the plain outlined button (error screen),
     * `scope` renders the Hub/Node identity badge as the dropdown trigger and
     * `scope-compact` its single-line variant.
     */
    variant?: "button" | "scope" | "scope-compact";
  }

  let { class: className, align = "start", variant = "button" }: Props = $props();

  let addOpen = $state(false);
  let newLabel = $state("");
  let newUrl = $state("");
  let push = $state<PushCapability | null>(null);
  let pushBusy = $state(false);

  const roleLabel = $derived(
    connection.role === "hub"
      ? m.connection_role_hub()
      : connection.role === "local_node"
        ? m.connection_role_local_node()
        : m.connection_role_disconnected(),
  );
  const originLabel = $derived(connection.origin || m.connection_same_origin());

  // Push capability depends on the connected backend, so check it on first mount
  onMount(() => {
    void refreshPush();
  });

  async function refreshPush() {
    try {
      push = await pushCapability();
    } catch {
      push = null;
    }
  }

  async function togglePush() {
    if (push === null || pushBusy) return;
    pushBusy = true;
    try {
      if (push.subscribed) {
        await disablePushNotifications();
        toast(m.connection_push_disabled_toast());
      } else {
        await enablePushNotifications();
        toast.success(m.connection_push_enabled_toast());
      }
      await refreshPush();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pushBusy = false;
    }
  }

  function addTarget(event: SubmitEvent) {
    event.preventDefault();
    if (!newUrl.trim()) return;
    try {
      const parsed = new URL(newUrl.trim());
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        throw new Error(m.connection_url_scheme_error());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : m.connection_url_invalid());
      return;
    }
    connection.addTarget(newLabel, newUrl);
    addOpen = false;
    newLabel = "";
    newUrl = "";
    toast.success(m.connection_target_added());
  }

  async function logout() {
    try {
      await logoutAndStop();
      toast(m.connection_logged_out());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger>
    {#snippet child({ props })}
      {#if variant === "button"}
        <Button
          variant="outline"
          size="sm"
          {...props}
          class={cn("gap-1.5 text-[13px]", className)}
          title={m.connection_current_target_tooltip({ role: roleLabel, origin: originLabel })}
        >
          <div class="flex items-center gap-1.5 min-w-0">
            <ServerIcon class="size-3.5 shrink-0 text-muted-foreground" />
            <span class="truncate">{roleLabel}</span>
          </div>
          <ChevronDownIcon class="size-3 text-muted-foreground/70 shrink-0 ml-auto" />
        </Button>
      {:else}
        <button
          {...props}
          class={cn(
            "group relative block w-full rounded-md text-left outline-none transition-opacity hover:opacity-85 focus-visible:ring-2 focus-visible:ring-ring/40",
            className,
          )}
          title={m.connection_current_target_tooltip({ role: roleLabel, origin: originLabel })}
          aria-label={m.connection_current_target()}
        >
          <ConnectionScopeBadge
            compact={variant === "scope-compact"}
            class={variant === "scope" ? "w-full pr-6" : "w-full"}
          />
          {#if variant === "scope"}
            <ChevronDownIcon
              class="text-muted-foreground/70 pointer-events-none absolute top-1/2 right-2 size-3 -translate-y-1/2 transition-transform group-data-[state=open]:rotate-180"
            />
          {/if}
        </button>
      {/if}
    {/snippet}
  </DropdownMenu.Trigger>
  <DropdownMenu.Content {align} class="w-80">
    <DropdownMenu.Label>{m.connection_current_target()}</DropdownMenu.Label>
    <DropdownMenu.Item disabled>
      <span class="truncate">{originLabel}</span>
      <span class="text-muted-foreground ml-auto text-[13px]">{roleLabel}</span>
    </DropdownMenu.Item>
    {#if sync.lastSyncedAt !== null}
      <DropdownMenu.Item disabled>
        <ClockIcon />
        <span>{m.shell_synced_relative({ time: formatRelativeTime(sync.lastSyncedAt) })}</span>
      </DropdownMenu.Item>
    {/if}
    <DropdownMenu.Separator />
    {#if connection.availableTargets.length > 0}
      <DropdownMenu.Label>{m.connection_targets_label()}</DropdownMenu.Label>
      {#each connection.availableTargets as target (target.id)}
        <DropdownMenu.Item onSelect={() => connection.switchTo(target.id)}>
          <CableIcon />
          <span class="flex-1 truncate">{target.label}</span>
          <span class="text-muted-foreground text-[13px]">{target.url}</span>
        </DropdownMenu.Item>
      {/each}
      <DropdownMenu.Separator />
    {/if}
    <DropdownMenu.Item onSelect={() => (addOpen = true)}>
      <PlusIcon />
      {m.connection_add_target_example()}
    </DropdownMenu.Item>
    {#if push !== null && push.supported && push.serverEnabled}
      <DropdownMenu.Separator />
      <DropdownMenu.Item disabled={pushBusy} onSelect={() => void togglePush()}>
        {#if push.subscribed}
          <BellOffIcon />
          {m.connection_push_disable()}
        {:else}
          <BellIcon />
          {m.connection_push_enable()}
        {/if}
      </DropdownMenu.Item>
    {/if}
    <DropdownMenu.Item onSelect={logout}>
      <LogOutIcon />
      {m.connection_logout()}
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<Dialog.Root bind:open={addOpen}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.connection_add_target_title()}</Dialog.Title>
      <Dialog.Description>
        {m.connection_add_target_description()}
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={addTarget}>
      <div class="grid gap-2">
        <Label for="target-label">{m.connection_display_name()}</Label>
        <Input
          id="target-label"
          bind:value={newLabel}
          placeholder={m.connection_display_name_placeholder()}
        />
      </div>
      <div class="grid gap-2">
        <Label for="target-url">URL</Label>
        <Input id="target-url" bind:value={newUrl} placeholder={m.connection_url_placeholder()} />
      </div>
      <Dialog.Footer>
        <Button type="submit" disabled={newUrl.trim() === ""}>{m.connection_add()}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
