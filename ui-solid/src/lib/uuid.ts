/**
 * RFC 4122 version 4 UUID.
 *
 * `crypto.randomUUID()` is only available in secure contexts. The Web UI is
 * frequently served over plain HTTP on a LAN / Tailscale address, where that
 * API is `undefined` even though `crypto.getRandomValues()` is available.
 * Command IDs only need uniqueness, so fall back to a v4 UUID built from
 * `getRandomValues` (and, as a last resort, `Math.random`).
 */
export function randomUuid(): string {
  const cryptoApi = globalThis.crypto;
  if (typeof cryptoApi?.randomUUID === "function") {
    return cryptoApi.randomUUID();
  }

  const bytes = new Uint8Array(16);
  if (typeof cryptoApi?.getRandomValues === "function") {
    cryptoApi.getRandomValues(bytes);
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256);
    }
  }

  // Set the version (4) and variant (10xx) bits.
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;

  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
