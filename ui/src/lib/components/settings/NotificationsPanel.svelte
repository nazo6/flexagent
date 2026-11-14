<script lang="ts">
  import { onMount } from "svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import {
    disablePushNotifications,
    enablePushNotifications,
    pushCapability,
    type PushCapability,
  } from "#lib/push.ts";
  import { connection } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import BellIcon from "@lucide/svelte/icons/bell";
  import BellOffIcon from "@lucide/svelte/icons/bell-off";
  import ExternalLinkIcon from "@lucide/svelte/icons/external-link";

  let { vapidSubjectHref }: { vapidSubjectHref?: string } = $props();

  let push = $state<PushCapability | null>(null);
  let loading = $state(true);
  let pushBusy = $state(false);

  const roleLabel = $derived(
    connection.role === "hub"
      ? m.connection_role_hub()
      : connection.role === "local_node"
        ? m.connection_role_local_node()
        : m.connection_role_disconnected(),
  );
  const originLabel = $derived(connection.origin || m.connection_same_origin());

  const browserSupportLabel = $derived.by(() => {
    if (!push) return "";
    if (!push.secureContext) return m.settings_notifications_unsupported_insecure();
    if (!push.supported) return m.settings_notifications_unsupported_browser();
    return m.settings_notifications_supported();
  });

  const permissionLabel = $derived.by(() => {
    if (!push) return "";
    if (push.permission === "granted") return m.settings_notifications_permission_granted();
    if (push.permission === "denied") return m.settings_notifications_permission_denied();
    return m.settings_notifications_permission_default();
  });

  const canToggle = $derived(
    !loading && !pushBusy && push !== null && push.supported && push.serverEnabled,
  );

  onMount(() => {
    void refreshPush();
  });

  async function refreshPush(): Promise<void> {
    loading = true;
    try {
      push = await pushCapability();
    } catch {
      push = null;
    } finally {
      loading = false;
    }
  }

  async function togglePush(): Promise<void> {
    if (!canToggle || push === null) return;
    pushBusy = true;
    try {
      if (push.subscribed) {
        await disablePushNotifications();
        toast(m.settings_notifications_disabled_toast());
      } else {
        await enablePushNotifications();
        toast.success(m.settings_notifications_enabled_toast());
      }
      await refreshPush();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      pushBusy = false;
    }
  }
</script>

<Card class="gap-4 p-5">
  <div class="flex flex-col gap-1">
    <h2 class="text-sm font-semibold">{m.settings_notifications_title()}</h2>
    <p class="text-muted-foreground text-[13px]">
      {m.settings_notifications_description()}
    </p>
  </div>

  {#if loading && push === null}
    <p class="text-muted-foreground text-[13px]">{m.settings_notifications_loading()}</p>
  {:else}
    <dl class="grid gap-2 text-[13px] sm:grid-cols-[11rem_1fr]">
      <dt class="text-muted-foreground">{m.settings_notifications_target_label()}</dt>
      <dd class="font-medium break-all">{roleLabel} ({originLabel})</dd>

      {#if push !== null}
        <dt class="text-muted-foreground">{m.settings_notifications_browser_support_label()}</dt>
        <dd>{browserSupportLabel}</dd>

        <dt class="text-muted-foreground">{m.settings_notifications_server_label()}</dt>
        <dd>
          {push.serverEnabled
            ? m.settings_notifications_server_ready()
            : m.settings_notifications_server_unavailable()}
        </dd>

        <dt class="text-muted-foreground">{m.settings_notifications_permission_label()}</dt>
        <dd>{permissionLabel}</dd>

        <dt class="text-muted-foreground">{m.settings_notifications_subscription_label()}</dt>
        <dd>
          <Badge variant={push.subscribed ? "default" : "secondary"}>
            {push.subscribed
              ? m.settings_notifications_subscribed()
              : m.settings_notifications_not_subscribed()}
          </Badge>
        </dd>
      {/if}
    </dl>

    {#if push?.permission === "denied"}
      <p class="text-muted-foreground text-[13px]">
        {m.settings_notifications_permission_denied_hint()}
      </p>
    {/if}

    <div class="flex flex-wrap items-center gap-2 pt-1">
      <Button
        type="button"
        variant={push?.subscribed ? "outline" : "default"}
        size="sm"
        disabled={!canToggle}
        onclick={() => void togglePush()}
      >
        {#if push?.subscribed}
          <BellOffIcon class="size-3.5" />
          {m.settings_notifications_disable()}
        {:else}
          <BellIcon class="size-3.5" />
          {m.settings_notifications_enable()}
        {/if}
      </Button>

      {#if vapidSubjectHref && connection.role !== null}
        <Button href={vapidSubjectHref} variant="ghost" size="sm">
          {m.settings_notifications_vapid_link()}
        </Button>
      {/if}
    </div>
  {/if}
</Card>

{#if connection.availableTargets.length > 0}
  <Card class="gap-3 p-5">
    <div class="flex flex-col gap-1">
      <h2 class="text-sm font-semibold">{m.settings_notifications_other_targets_title()}</h2>
      <p class="text-muted-foreground text-[13px]">
        {m.settings_notifications_other_targets_note()}
      </p>
    </div>
    <ul class="divide-border divide-y text-[13px]">
      {#each connection.availableTargets as target (target.id)}
        <li class="flex items-center justify-between gap-3 py-2 first:pt-0 last:pb-0">
          <div class="min-w-0">
            <div class="truncate font-medium">{target.label}</div>
            <div class="text-muted-foreground truncate text-[12px]">{target.url}</div>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onclick={() => connection.switchTo(target.id)}
          >
            <ExternalLinkIcon class="size-3.5" />
            {m.settings_notifications_open_target()}
          </Button>
        </li>
      {/each}
    </ul>
  </Card>
{/if}
