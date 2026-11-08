import { describe, expect, it } from "vitest";
import { decodeBase64ToBytes, decodeBase64ToText, encodeTextToBase64 } from "./base64";

describe("base64 helpers", () => {
  it("round-trips ASCII text", () => {
    expect(decodeBase64ToText(encodeTextToBase64("cargo test --workspace"))).toBe(
      "cargo test --workspace",
    );
  });

  it("round-trips multi-byte text (Japanese / emoji)", () => {
    const text = "こんにちは 🌊 FlexAgent"; // allow-japanese: multi-byte (CJK/emoji) round-trip test data
    expect(decodeBase64ToText(encodeTextToBase64(text))).toBe(text);
  });

  it("decodes raw bytes", () => {
    const bytes = decodeBase64ToBytes(btoa("AB"));
    expect([...bytes]).toEqual([65, 66]);
  });
});
