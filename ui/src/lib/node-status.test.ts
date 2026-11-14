import { describe, expect, it } from "vitest";
import { hubLinkLabel } from "./node-status.ts";

describe("hubLinkLabel", () => {
  it("reports standalone cleanly instead of disconnected", () => {
    expect(hubLinkLabel("standalone")).toBe("Standalone");
    expect(hubLinkLabel("connected")).toBe("Connected");
    expect(hubLinkLabel("connecting")).toBe("Connecting");
  });
});
