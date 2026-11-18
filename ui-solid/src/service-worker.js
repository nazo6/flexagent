/**
 * FlexAgent PWA service worker.
 *
 * The build (see `vite.config.ts`) replaces the version and precache
 * placeholders below with a fresh cache version and the emitted file list.
 *
 * - App Shell cache: the UI starts offline; navigation is network-first.
 * - Web Push: shows VAPID notifications and lets the banner approve or reject
 *   a permission without opening the app.
 */

/* eslint-disable no-undef */

const VERSION = __FXG_VERSION__;
const APP_SHELL = __FXG_PRECACHE__;

const CACHE_NAME = `fxg-${VERSION}`;

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE_NAME);
      // Cache each asset independently: one missing file must not abort the
      // whole install and leave the old worker active forever.
      await Promise.allSettled(APP_SHELL.map((path) => cache.add(path)));
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      // Keep the current cache and the previous one: an open tab may lazy-load
      // a chunk from the previous build after this worker activates.
      const fxgKeys = keys.filter((key) => key.startsWith("fxg-")).sort();
      const keep = new Set([CACHE_NAME, ...fxgKeys.slice(-2)]);
      await Promise.all(keys.filter((key) => !keep.has(key)).map((key) => caches.delete(key)));
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.origin !== self.location.origin) return;
  // API / WS always go to the network (never cached).
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

/* ------------------------------------------------------------------ */
/* Web Push                                                            */
/* ------------------------------------------------------------------ */

self.addEventListener("push", (event) => {
  const data = event.data;
  if (!data) return;
  let payload;
  try {
    payload = data.json();
  } catch {
    return;
  }

  const actions = [];
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
  const payload = event.notification.data;
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

/** Respond to a permission straight from the notification (Cookie auth). */
async function respondToPermission(payload, optionId) {
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
      // 409 = ALREADY_RESOLVED is idempotent success.
      await self.registration.showNotification("Approval sent", {
        body: payload.body,
        tag: payload.tag,
        icon: "/icon-192.png",
        silent: true,
      });
      return;
    }
    await showFailure(payload, `Response failed (HTTP ${response.status})`);
  } catch {
    await showFailure(payload, "Could not reach the network. Open the app and try again.");
  }
}

async function showFailure(payload, message) {
  await self.registration.showNotification("Failed to send approval", {
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

/** Focus an existing app window, or open a new one. */
async function openApp(sessionId) {
  const target = sessionId ? `/sessions/${sessionId}` : "/";
  const windows = await self.clients.matchAll({
    type: "window",
    includeUncontrolled: true,
  });
  for (const client of windows) {
    if (new URL(client.url).origin === self.location.origin) {
      await client.focus();
      if (sessionId !== undefined) {
        try {
          await client.navigate(target);
        } catch (error) {
          console.error("[fxg] failed to navigate the app window", error);
        }
      }
      return;
    }
  }
  await self.clients.openWindow(target);
}

/** Re-subscribe and re-register with the server when the subscription expires. */
self.addEventListener("pushsubscriptionchange", (event) => {
  event.waitUntil(
    (async () => {
      try {
        const info = await fetch("/api/v1/system/info", { credentials: "same-origin" });
        if (!info.ok) {
          console.error("[fxg] failed to load system info for push re-subscription", info.status);
          return;
        }
        const body = await info.json();
        if (!body.vapid_public_key) {
          console.error("[fxg] VAPID public key is not available");
          return;
        }
        const applicationServerKey = urlBase64ToUint8Array(body.vapid_public_key);
        const oldSubscription = event.oldSubscription;
        if (oldSubscription !== undefined) {
          try {
            await oldSubscription.unsubscribe();
          } catch (error) {
            console.error("[fxg] failed to unsubscribe the old push subscription", error);
          }
        }
        const existing = await self.registration.pushManager.getSubscription();
        const subscription =
          existing ??
          (await self.registration.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey,
          }));
        const keys = subscription.toJSON().keys ?? {};
        if (!keys.p256dh || !keys.auth) {
          console.error("[fxg] push subscription has no encryption keys");
          return;
        }
        const response = await fetch("/api/v1/push/subscribe", {
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
        if (!response.ok) {
          throw new Error(`push subscribe failed: ${response.status}`);
        }
      } catch (error) {
        console.error("[fxg] push re-subscription failed", error);
      }
    })(),
  );
});

/** VAPID public keys are base64url; PushManager wants a Uint8Array. */
function urlBase64ToUint8Array(base64String) {
  const padding = "=".repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, "+").replace(/_/g, "/");
  const rawData = atob(base64);
  const outputArray = new Uint8Array(rawData.length);
  for (let i = 0; i < rawData.length; ++i) {
    outputArray[i] = rawData.charCodeAt(i);
  }
  return outputArray;
}
