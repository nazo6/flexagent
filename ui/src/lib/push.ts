import type { SystemInfoResponse } from "$lib/generated/SystemInfoResponse";
import { connection } from "$lib/stores/app.svelte";
import { isValidVapidKey, urlBase64ToUint8Array } from "./push-keys";

/**
 * Android / Desktop PWA の Web Push 通知。
 *
 * 中央サーバーの VAPID 公開鍵 (`GET /api/v1/system/info`) で購読し、
 * `POST /api/v1/push/subscribe` に登録する。通知バナー上の Approve / Reject は
 * Service Worker (`src/service-worker.ts`) が直接 API を叩く。
 */

export interface PushCapability {
  /** このブラウザが Service Worker / Push API に対応しているか */
  supported: boolean;
  /** 通知許可の状態 */
  permission: NotificationPermission;
  /** 現在このオリジンで購読済みか */
  subscribed: boolean;
  /** サーバーが VAPID 公開鍵を配布しているか */
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

/** 現在の Web Push 対応状況を取得する。 */
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
 * Web Push 通知を有効化する (通知許可 → 購読 → サーバー登録)。
 *
 * 失敗時は日本語メッセージの `Error` を投げる。
 */
export async function enablePushNotifications(): Promise<void> {
  if (!hasPushSupport()) {
    throw new Error("このブラウザは Web Push に対応していません");
  }
  const info = await systemInfo();
  const vapidKey = info?.vapid_public_key ?? null;
  if (vapidKey === null) {
    throw new Error("接続先サーバーでは Web Push が利用できません (中央サーバーのみ)");
  }
  const applicationServerKey = urlBase64ToUint8Array(vapidKey);
  if (!isValidVapidKey(applicationServerKey)) {
    throw new Error("VAPID 公開鍵が不正です (サーバーの vapid.json を確認してください)");
  }

  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error("通知の許可が得られませんでした");
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
    throw new Error("購読情報 (p256dh / auth) を取得できませんでした");
  }
  await connection.client.pushSubscribe({
    endpoint: subscription.endpoint,
    p256dh,
    auth,
    device_name: navigator.userAgent,
  });
}

/** この端末の購読を解除する (サーバー側は 404/410 で自然に失効する)。 */
export async function disablePushNotifications(): Promise<void> {
  if (!hasPushSupport()) return;
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  await subscription?.unsubscribe();
}
