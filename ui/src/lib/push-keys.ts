import { decodeBase64ToBytes } from "./base64";

/**
 * base64url (no padding) 文字列をバイト列へデコードする。
 *
 * Web Push の `applicationServerKey` (VAPID 公開鍵) は base64url 形式で
 * 渡されるため、標準 base64 へ変換してからデコードする。
 */
export function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  const padded = base64
    .replace(/-/g, "+")
    .replace(/_/g, "/")
    .padEnd(Math.ceil(base64.length / 4) * 4, "=");
  return decodeBase64ToBytes(padded);
}

/** VAPID 公開鍵として妥当な長さか (非圧縮 P-256 点 = 65 バイト)。 */
export function isValidVapidKey(bytes: Uint8Array): boolean {
  return bytes.length === 65 && bytes[0] === 0x04;
}
