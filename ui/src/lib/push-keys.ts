import { base64UrlToBytes } from "../service-worker/format.ts";

/**
 * The VAPID public key arrives as base64url. The decoding lives in the
 * dependency-free `src/service-worker/format.ts`, shared with the service
 * worker (whose build inlines the same function into `sw.js`).
 */
export function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  return base64UrlToBytes(base64) as Uint8Array<ArrayBuffer>;
}

/** Whether the bytes have a valid VAPID public key length (uncompressed P-256 point = 65 bytes). */
export function isValidVapidKey(bytes: Uint8Array): boolean {
  return bytes.length === 65 && bytes[0] === 0x04;
}
