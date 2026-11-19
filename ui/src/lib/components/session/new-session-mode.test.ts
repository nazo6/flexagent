import { describe, expect, it } from "vitest";
import { probeInitialConfig, sessionModeFor, sessionPath } from "./new-session-mode.ts";

const MODES = [
  { mode_id: "build", name: "Build", description: null },
  { mode_id: "plan", name: "Plan", description: null },
];

/**
 * Modes are agent-specific: sending one the agent does not report fails the
 * whole `session/set_mode` call, so it degrades to the agent default instead.
 */
describe("sessionModeFor", () => {
  it("keeps a mode the agent reports", () => {
    expect(sessionModeFor("plan", MODES)).toBe("plan");
  });

  it("falls back to the default for a mode the agent does not report", () => {
    expect(sessionModeFor("build", MODES.slice(1))).toBe("default");
  });

  it("passes the choice through until the probe answers", () => {
    expect(sessionModeFor("build", null)).toBe("build");
    expect(sessionModeFor("default", MODES)).toBe("default");
  });
});

describe("probeInitialConfig", () => {
  it("sends nothing before the probe for the current target", () => {
    expect(probeInitialConfig({ probeReady: false, values: { model: "x" } })).toEqual([]);
  });

  it("sends the selected options once the probe is ready", () => {
    expect(probeInitialConfig({ probeReady: true, values: { model: "x" } })).toEqual([
      { key: "model", value: "x" },
    ]);
  });
});

describe("sessionPath", () => {
  it("points at the session detail route", () => {
    expect(sessionPath("s1")).toBe("/sessions/s1");
  });
});
