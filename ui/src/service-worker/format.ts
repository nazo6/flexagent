/**
 * Formatting helpers shared by the service worker and the app.
 *
 * The build inlines the bodies of the functions the worker imports into
 * `sw.js` (see `vite.config.ts`), so this file must stay dependency-free: no
 * imports, no app modules, only functions that run in a worker scope.
 */

/** Decode a base64url (no padding) string into bytes. */
export function base64UrlToBytes(base64Url: string): Uint8Array<ArrayBuffer> {
  const binary = atob(toBase64(base64Url));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/** Convert base64url (as used by VAPID keys) into padded standard base64. */
export function toBase64(base64Url: string): string {
  return base64Url
    .replace(/-/g, "+")
    .replace(/_/g, "/")
    .padEnd(Math.ceil(base64Url.length / 4) * 4, "=");
}

/** Collapse text into a single line for a notification body. */
export function toNotificationBody(text: string, maxLength = 240): string {
  const collapsed = text.replace(/\s+/g, " ").trim();
  return collapsed.length <= maxLength ? collapsed : `${collapsed.slice(0, maxLength - 1)}…`;
}
