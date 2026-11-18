/**
 * Conversion between Base64 and UTF-8 text / byte arrays.
 *
 * The PTY WS (`PtyClientMessage` / `PtyServerMessage`) and `TerminalOutput`
 * events carry byte arrays as Base64.
 */

/** Decode a Base64 string into bytes. */
export function decodeBase64ToBytes(dataB64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(dataB64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/** Decode a Base64 string into UTF-8 text (invalid bytes are replaced). */
export function decodeBase64ToText(dataB64: string): string {
  return new TextDecoder().decode(decodeBase64ToBytes(dataB64));
}

/** Encode UTF-8 text into Base64 (used to send PTY key input). */
export function encodeTextToBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

/** Encode a byte array into Base64. */
export function encodeBytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}
