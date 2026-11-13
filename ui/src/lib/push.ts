import { clientId } from "#lib/client-identity.ts";
import type { SystemInfoResponse } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { connection } from "#lib/stores/app.svelte.ts";
import { isValidVapidKey, urlBase64ToUint8Array } from "./push-keys";

/**
 * Web Push notifications for Android / Desktop PWAs.
 *
 * Subscribes with the central server's VAPID public key
 * (`GET /api/v1/system/info`) and registers via
 * `POST /api/v1/push/subscribe`. The Approve / Reject actions on the notification
 * banner call the API directly from the Service Worker (`src/service-worker.ts`).
 */

/** Whether the current subscription has been registered with this client ID. */
let registrationSynced = false;

export interface PushCapability {
  /** Whether this browser supports Service Worker / Push API */
  supported: boolean;
  /** Notification permission state */
  permission: NotificationPermission;
  /** Whether this origin is currently subscribed */
  subscribed: boolean;
  /** Whether the server distributes a VAPID public key */
  serverEnabled: boolean;
}

function hasPushSupport(): boolean {
  return (
    typeof navigator !== "undefined" &&
    "serviceWorker" in navigator &&
    typeof PushManager !== "undefined"
  );
}

async function systemInfo(): Promise<SystemInfoResponse | null> {
  try {
    return await connection.client.systemInfo();
  } catch {
    return null;
  }
}

/** Get the current Web Push support status. */
export async function pushCapability(): Promise<PushCapability> {
  const info = await systemInfo();
  const serverEnabled = (info?.vapid_public_key ?? null) !== null;
  if (!hasPushSupport()) {
    return {
      supported: false,
      permission: "denied",
      subscribed: false,
      serverEnabled,
    };
  }
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  return {
    supported: true,
    permission: Notification.permission,
    subscribed: subscription !== null,
    serverEnabled,
  };
}

/**
 * Enable Web Push notifications (permission → subscribe → server registration).
 *
 * Throws an `Error` with a localized message on failure.
 */
export async function enablePushNotifications(): Promise<void> {
  if (!hasPushSupport()) {
    throw new Error(m.push_unsupported_browser());
  }
  const info = await systemInfo();
  const vapidKey = info?.vapid_public_key ?? null;
  if (vapidKey === null) {
    throw new Error(m.push_hub_unavailable());
  }
  const applicationServerKey = urlBase64ToUint8Array(vapidKey);
  if (!isValidVapidKey(applicationServerKey)) {
    throw new Error(m.push_invalid_vapid_key());
  }

  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error(m.push_permission_denied());
  }

  const registration = await navigator.serviceWorker.ready;
  const subscription =
    (await registration.pushManager.getSubscription()) ??
    (await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey,
    }));

  const keys = subscription.toJSON().keys ?? {};
  const p256dh = keys.p256dh;
  const auth = keys.auth;
  if (!p256dh || !auth) {
    throw new Error(m.push_missing_subscription_keys());
  }
  await connection.client.pushSubscribe({
    endpoint: subscription.endpoint,
    p256dh,
    auth,
    device_name: navigator.userAgent,
    client_id: clientId(),
  });
  registrationSynced = true;
}

/**
 * Re-register an existing subscription with the server.
 *
 * This backfills `client_id` for subscriptions created before client identities
 * existed and keeps the device association current after an upgrade.
 */
export async function syncPushSubscription(): Promise<void> {
  if (registrationSynced) return;
  if (
    !hasPushSupport() ||
    typeof Notification === "undefined" ||
    Notification.permission !== "granted"
  )
    return;
  const info = await systemInfo();
  if (info?.vapid_public_key == null) return;

  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  if (subscription === null) return;
  const keys = subscription.toJSON().keys ?? {};
  if (!keys.p256dh || !keys.auth) return;

  await connection.client.pushSubscribe({
    endpoint: subscription.endpoint,
    p256dh: keys.p256dh,
    auth: keys.auth,
    device_name: navigator.userAgent,
    client_id: clientId(),
  });
  registrationSynced = true;
}

/** Unsubscribe this device (the server entry expires naturally on 404/410). */
export async function disablePushNotifications(): Promise<void> {
  if (!hasPushSupport()) return;
  registrationSynced = false;
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  await subscription?.unsubscribe();
}
