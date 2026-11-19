import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
import IconBell from "~icons/lucide/bell";
import IconBellOff from "~icons/lucide/bell-off";
import IconExternalLink from "~icons/lucide/external-link";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  disablePushNotifications,
  enablePushNotifications,
  pushCapability,
  type PushCapability,
} from "#lib/push.ts";
import { connection } from "#lib/stores/app.ts";
import { errorMessage } from "#lib/api/error-messages.ts";

/** Web Push capability / subscription state for this browser. */
export default function NotificationsPanel(props: { vapidSubjectHref?: string }) {
  const [push, setPush] = createSignal<PushCapability | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [pushBusy, setPushBusy] = createSignal(false);

  const roleLabel = createMemo(() =>
    connection.role === "hub"
      ? m.connection_role_hub()
      : connection.role === "local_node"
        ? m.connection_role_local_node()
        : m.connection_role_disconnected(),
  );
  const originLabel = () => connection.origin || m.connection_same_origin();

  const browserSupportLabel = createMemo(() => {
    const current = push();
    if (!current) return "";
    if (!current.secureContext) return m.settings_notifications_unsupported_insecure();
    if (!current.supported) return m.settings_notifications_unsupported_browser();
    return m.settings_notifications_supported();
  });

  const permissionLabel = createMemo(() => {
    const current = push();
    if (!current) return "";
    if (current.permission === "granted") return m.settings_notifications_permission_granted();
    if (current.permission === "denied") return m.settings_notifications_permission_denied();
    return m.settings_notifications_permission_default();
  });

  const canToggle = createMemo(() => {
    const current = push();
    return (
      !loading() && !pushBusy() && current !== null && current.supported && current.serverEnabled
    );
  });

  onSettled(() => {
    void refreshPush();
  });

  async function refreshPush(): Promise<void> {
    setLoading(true);
    try {
      setPush(await pushCapability());
    } catch {
      setPush(null);
    } finally {
      setLoading(false);
    }
  }

  async function togglePush(): Promise<void> {
    const current = push();
    if (!canToggle() || current === null) return;
    setPushBusy(true);
    try {
      if (current.subscribed) {
        await disablePushNotifications();
        toast(m.settings_notifications_disabled_toast());
      } else {
        await enablePushNotifications();
        toast.success(m.settings_notifications_enabled_toast());
      }
      await refreshPush();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setPushBusy(false);
    }
  }

  return (
    <>
      <Card class="gap-4 p-5">
        <div class="flex flex-col gap-1">
          <h2 class="text-sm font-semibold">{m.settings_notifications_title()}</h2>
          <p class="text-muted-foreground text-[13px]">{m.settings_notifications_description()}</p>
        </div>

        <Show
          when={!(loading() && push() === null)}
          fallback={
            <p class="text-muted-foreground text-[13px]">{m.settings_notifications_loading()}</p>
          }
        >
          <dl class="grid gap-2 text-[13px] sm:grid-cols-[11rem_1fr]">
            <dt class="text-muted-foreground">{m.settings_notifications_target_label()}</dt>
            <dd class="font-medium break-all">
              {roleLabel()} ({originLabel()})
            </dd>

            <Show when={push()}>
              {(current) => (
                <>
                  <dt class="text-muted-foreground">
                    {m.settings_notifications_browser_support_label()}
                  </dt>
                  <dd>{browserSupportLabel()}</dd>

                  <dt class="text-muted-foreground">{m.settings_notifications_server_label()}</dt>
                  <dd>
                    {current().serverEnabled
                      ? m.settings_notifications_server_ready()
                      : m.settings_notifications_server_unavailable()}
                  </dd>

                  <dt class="text-muted-foreground">
                    {m.settings_notifications_permission_label()}
                  </dt>
                  <dd>{permissionLabel()}</dd>

                  <dt class="text-muted-foreground">
                    {m.settings_notifications_subscription_label()}
                  </dt>
                  <dd>
                    <Badge variant={current().subscribed ? "default" : "secondary"}>
                      {current().subscribed
                        ? m.settings_notifications_subscribed()
                        : m.settings_notifications_not_subscribed()}
                    </Badge>
                  </dd>
                </>
              )}
            </Show>
          </dl>

          <Show when={push()?.permission === "denied"}>
            <p class="text-muted-foreground text-[13px]">
              {m.settings_notifications_permission_denied_hint()}
            </p>
          </Show>

          <div class="flex flex-wrap items-center gap-2 pt-1">
            <Button
              type="button"
              variant={push()?.subscribed ? "outline" : "default"}
              size="sm"
              disabled={!canToggle()}
              onClick={() => void togglePush()}
            >
              <Show
                when={push()?.subscribed}
                fallback={
                  <>
                    <IconBell class="size-3.5" />
                    {m.settings_notifications_enable()}
                  </>
                }
              >
                <IconBellOff class="size-3.5" />
                {m.settings_notifications_disable()}
              </Show>
            </Button>

            <Show when={props.vapidSubjectHref && connection.role !== null}>
              <Button href={props.vapidSubjectHref} variant="ghost" size="sm">
                {m.settings_notifications_vapid_link()}
              </Button>
            </Show>
          </div>
        </Show>
      </Card>

      <Show when={connection.availableTargets.length > 0}>
        <Card class="gap-3 p-5">
          <div class="flex flex-col gap-1">
            <h2 class="text-sm font-semibold">{m.settings_notifications_other_targets_title()}</h2>
            <p class="text-muted-foreground text-[13px]">
              {m.settings_notifications_other_targets_note()}
            </p>
          </div>
          <ul class="divide-border divide-y text-[13px]">
            <For each={connection.availableTargets}>
              {(target) => (
                <li class="flex items-center justify-between gap-3 py-2 first:pt-0 last:pb-0">
                  <div class="min-w-0">
                    <div class="truncate font-medium">{target.label}</div>
                    <div class="text-muted-foreground truncate text-[12px]">{target.url}</div>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => connection.switchTo(target.id)}
                  >
                    <IconExternalLink class="size-3.5" />
                    {m.settings_notifications_open_target()}
                  </Button>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>
    </>
  );
}
