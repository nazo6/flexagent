/// <reference types="@sveltejs/kit" />
/// <reference no-default-lib="true" />
/// <reference lib="esnext" />
/// <reference lib="webworker" />

/**
 * FlexAgent PWA Service Worker。
 *
 * - **App Shell キャッシュ**: ビルド成果物と静的ファイルを `$service-worker`
 *   からキャッシュし、オフライン / モバイル回線不安定時でも UI を起動できる
 *   (ナビゲーションはネットワーク優先のため開発時の HMR も妨げない)。
 * - **Web Push 受信**: 中央サーバーからの VAPID 通知を表示し、通知バナーの
 *   `[Approve]` / `[Reject]` から
 *   `POST /api/v1/sessions/:id/permissions/:req_id/respond` を直接叩く
 *   (アプリを開かずに承認が完了する)。
 */

import { build, files, version } from "$service-worker";
import { urlBase64ToUint8Array } from "$lib/push-keys";

declare const self: ServiceWorkerGlobalScope;

/** サーバーの `PushNotificationPayload` と対応するペイロード。 */
interface PushPayload {
  title: string;
  body: string;
  session_id: string;
  request_id: string;
  tag: string;
  allow_option_id: string | null;
  reject_option_id: string | null;
}

const CACHE_NAME = `fxg-${version}`;
const APP_SHELL = [...build, ...files];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      await cache.addAll(APP_SHELL);
      // SPA フォールバック (adapter-static の index.html) を保持する
      try {
        await cache.add("/index.html");
      } catch {
        /* 開発ビルド等で存在しない場合は無視 */
      }
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key)));
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) return;
  // API / WS は常にネットワークへ (オフラインキャッシュしない)
  if (url.pathname.startsWith("/api/")) return;
  if (event.request.method !== "GET") return;

  if (event.request.mode === "navigate") {
    event.respondWith(
      (async () => {
        try {
          return await fetch(event.request);
        } catch {
          const cache = await caches.open(CACHE_NAME);
          return (await cache.match("/index.html")) ?? Response.error();
        }
      })(),
    );
    return;
  }

  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      const cached = await cache.match(event.request);
      if (cached) return cached;
      return fetch(event.request);
    })(),
  );
});

// ----------------------------------------------------------------------
// Web Push
// ----------------------------------------------------------------------

self.addEventListener("push", (event) => {
  const data = event.data;
  if (!data) return;
  let payload: PushPayload;
  try {
    payload = data.json() as PushPayload;
  } catch {
    return;
  }

  const actions: NotificationAction[] = [];
  if (payload.allow_option_id) actions.push({ action: "approve", title: "Approve" });
  if (payload.reject_option_id) actions.push({ action: "reject", title: "Reject" });

  event.waitUntil(
    self.registration.showNotification(payload.title, {
      body: payload.body,
      tag: payload.tag,
      icon: "/icon-192.png",
      badge: "/icon-192.png",
      data: payload,
      actions,
      requireInteraction: true,
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const payload = event.notification.data as PushPayload | undefined;
  const action = event.action;

  if (
    payload &&
    (action === "approve" || action === "reject") &&
    payload.session_id !== "" &&
    payload.request_id !== ""
  ) {
    const optionId = action === "approve" ? payload.allow_option_id : payload.reject_option_id;
    if (optionId) {
      event.waitUntil(respondToPermission(payload, optionId));
      return;
    }
  }

  event.waitUntil(openApp(payload?.session_id));
});

/** 通知バナーからの承認応答 (Cookie 認証で API を直接叩く)。 */
async function respondToPermission(payload: PushPayload, optionId: string): Promise<void> {
  const url =
    `/api/v1/sessions/${encodeURIComponent(payload.session_id)}` +
    `/permissions/${encodeURIComponent(payload.request_id)}/respond`;
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        selected_option_id: optionId,
        always: false,
        resolved_by: "android_push",
      }),
      credentials: "same-origin",
    });
    if (response.ok || response.status === 409) {
      // 409 = ALREADY_RESOLVED (冪等: 他のクライアントが解決済み) は正常終了
      await self.registration.showNotification("承認を送信しました", {
        body: payload.body,
        tag: payload.tag,
        icon: "/icon-192.png",
        silent: true,
      });
      return;
    }
    await showFailure(payload, `応答に失敗しました (HTTP ${response.status})`);
  } catch {
    await showFailure(
      payload,
      "ネットワークに接続できませんでした。アプリを開いて再試行してください。",
    );
  }
}

async function showFailure(payload: PushPayload, message: string): Promise<void> {
  await self.registration.showNotification("承認の送信に失敗", {
    body: message,
    tag: payload.tag,
    icon: "/icon-192.png",
    data: payload,
    actions: [
      ...(payload.allow_option_id ? [{ action: "approve", title: "Approve" }] : []),
      ...(payload.reject_option_id ? [{ action: "reject", title: "Reject" }] : []),
    ],
  });
}

/** 既存のアプリウィンドウをフォーカスするか、新しく開く。 */
async function openApp(sessionId: string | undefined): Promise<void> {
  const target = sessionId ? `/sessions/${sessionId}` : "/inbox";
  const windows = await self.clients.matchAll({
    type: "window",
    includeUncontrolled: true,
  });
  for (const client of windows) {
    if (new URL(client.url).origin === self.location.origin) {
      await client.focus();
      return;
    }
  }
  await self.clients.openWindow(target);
}

/** 購読が失効した場合に再購読し、サーバーへ再登録する (ベストエフォート)。 */
self.addEventListener("pushsubscriptionchange", (event) => {
  event.waitUntil(
    (async () => {
      try {
        const info = await fetch("/api/v1/system/info", {
          credentials: "same-origin",
        });
        if (!info.ok) return;
        const body = (await info.json()) as { vapid_public_key: string | null };
        if (!body.vapid_public_key) return;
        const applicationServerKey = urlBase64ToUint8Array(body.vapid_public_key);
        const subscription = await self.registration.pushManager.subscribe({
          userVisibleOnly: true,
          applicationServerKey,
        });
        const keys = subscription.toJSON().keys ?? {};
        if (!keys.p256dh || !keys.auth) return;
        await fetch("/api/v1/push/subscribe", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "same-origin",
          body: JSON.stringify({
            endpoint: subscription.endpoint,
            p256dh: keys.p256dh,
            auth: keys.auth,
            device_name: self.navigator.userAgent,
          }),
        });
      } catch {
        /* 次回のアプリ起動時に再購読できる */
      }
    })(),
  );
});
