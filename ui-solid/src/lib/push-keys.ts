import { decodeBase64ToBytes } from "./base64";

/**
 * Decode a base64url (no padding) string into bytes.
 *
 * Web Push's `applicationServerKey` (VAPID public key) is provided in base64url
 * form, so convert it to standard base64 before decoding.
 */
export function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  const padded = base64
    .replace(/-/g, "+")
    .replace(/_/g, "/")
    .padEnd(Math.ceil(base64.length / 4) * 4, "=");
  return decodeBase64ToBytes(padded);
}

/** Whether the bytes have a valid VAPID public key length (uncompressed P-256 point = 65 bytes). */
export function isValidVapidKey(bytes: Uint8Array): boolean {
  return bytes.length === 65 && bytes[0] === 0x04;
}
