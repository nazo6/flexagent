/**
 * Base64 と UTF-8 テキスト / バイト列の相互変換。
 *
 * PTY WS (`PtyClientMessage` / `PtyServerMessage`) と `TerminalOutput` 系
 * イベントは Base64 でバイト列を運ぶ。
 */

/** Base64 文字列をバイト列へデコードする。 */
export function decodeBase64ToBytes(dataB64: string): Uint8Array {
  const binary = atob(dataB64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/** Base64 文字列を UTF-8 テキストへデコードする (不正バイトは置換される)。 */
export function decodeBase64ToText(dataB64: string): string {
  return new TextDecoder().decode(decodeBase64ToBytes(dataB64));
}

/** UTF-8 テキストを Base64 へエンコードする (PTY のキー入力送信用)。 */
export function encodeTextToBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

/** バイト列を Base64 へエンコードする。 */
export function encodeBytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}
