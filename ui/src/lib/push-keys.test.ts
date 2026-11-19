import { describe, expect, it } from "vitest";
import { base64UrlToBytes, toNotificationBody } from "../service-worker/format.ts";
import { urlBase64ToUint8Array } from "./push-keys.ts";

/** Uncompressed P-256 point: 0x04 followed by 64 bytes. */
function vapidKeyBytes(): Uint8Array {
  const bytes = new Uint8Array(65);
  bytes[0] = 0x04;
  for (let index = 1; index < bytes.length; index += 1) bytes[index] = index;
  return bytes;
}

function toBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/**
 * The service worker and the app decode the VAPID key with the same function
 * (the build inlines `format.ts` into `sw.js`); a regression shows up here
 * before it shows up as a browser-level `InvalidAccessError`.
 */
describe("base64url decoding", () => {
  it("decodes every padding remainder", () => {
    for (const length of [1, 2, 3, 4, 5, 6, 7, 8]) {
      const bytes = Uint8Array.from({ length }, (_, index) => index + 250);
      expect(base64UrlToBytes(toBase64Url(bytes))).toEqual(bytes);
    }
  });

  it("round-trips a VAPID key through both entry points", () => {
    const key = vapidKeyBytes();
    const encoded = toBase64Url(key);
    expect(base64UrlToBytes(encoded)).toEqual(key);
    expect(urlBase64ToUint8Array(encoded)).toEqual(key);
  });
});

describe("toNotificationBody", () => {
  it("collapses whitespace and ellipsizes long text", () => {
    expect(toNotificationBody("  two\n lines ")).toBe("two lines");
    const long = "x".repeat(300);
    const body = toNotificationBody(long, 10);
    expect(body).toHaveLength(10);
    expect(body.endsWith("…")).toBe(true);
  });
});
