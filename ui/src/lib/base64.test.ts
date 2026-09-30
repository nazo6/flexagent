import { describe, expect, it } from "vitest";
import { decodeBase64ToBytes, decodeBase64ToText, encodeTextToBase64 } from "./base64";

describe("base64 helpers", () => {
  it("round-trips ASCII text", () => {
    expect(decodeBase64ToText(encodeTextToBase64("cargo test --workspace"))).toBe(
      "cargo test --workspace",
    );
  });

  it("round-trips multi-byte text (日本語 / 絵文字)", () => {
    const text = "こんにちは 🌊 FlexAgent";
    expect(decodeBase64ToText(encodeTextToBase64(text))).toBe(text);
  });

  it("decodes raw bytes", () => {
    const bytes = decodeBase64ToBytes(btoa("AB"));
    expect([...bytes]).toEqual([65, 66]);
  });
});
