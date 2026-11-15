import { describe, expect, it } from "vitest";
import {
  browserSettingsPath,
  hubSettingsPath,
  isNodeTabAvailable,
  nodeSettingsPath,
  resolveDefaultNodeId,
  settingsHomePath,
  settingsPathForTarget,
} from "./settings-routes.ts";

describe("settings-routes", () => {
  it("hides node-local tabs when viewing a node via the hub", () => {
    expect(isNodeTabAvailable("process", false)).toBe(true);
    expect(isNodeTabAvailable("process", true)).toBe(false);
    expect(isNodeTabAvailable("audit", true)).toBe(false);
    expect(isNodeTabAvailable("agents", true)).toBe(true);
  });

  it("builds settings paths and encodes node ids", () => {
    expect(hubSettingsPath()).toBe("/settings/hub/general");
    expect(hubSettingsPath("tokens")).toBe("/settings/hub/tokens");
    expect(browserSettingsPath()).toBe("/settings/browser/appearance");
    expect(nodeSettingsPath("agents", "node/1")).toBe("/settings/nodes/node%2F1/agents");
    expect(nodeSettingsPath("agents", null)).toBe("/settings/nodes");
    expect(settingsHomePath(true, null)).toBe("/settings/hub/general");
    expect(settingsHomePath(false, "local-1")).toBe("/settings/nodes/local-1/general");
  });

  it("resolves default node id for hub and direct node", () => {
    const nodes = [
      { node_id: "off", is_online: false },
      { node_id: "on-1", is_online: true },
      { node_id: "on-2", is_online: true },
    ];
    expect(resolveDefaultNodeId(true, nodes, "hub")).toBe("on-1");
    expect(resolveDefaultNodeId(true, [{ node_id: "off", is_online: false }], "hub")).toBeNull();
    expect(resolveDefaultNodeId(false, nodes, "local-node")).toBe("local-node");
  });

  it("resolves search entry targets across current/hub/node/browser", () => {
    expect(settingsPathForTarget("current", "process", { isHub: true, nodeId: "n1" })).toBe(
      "/settings/hub/process",
    );
    expect(settingsPathForTarget("current", "process", { isHub: false, nodeId: "n1" })).toBe(
      "/settings/nodes/n1/process",
    );
    expect(settingsPathForTarget("node", "agents", { isHub: true, nodeId: null })).toBe(
      "/settings/nodes",
    );
    expect(settingsPathForTarget("browser", "notifications", { isHub: true, nodeId: "n1" })).toBe(
      "/settings/browser/notifications",
    );
  });
});
