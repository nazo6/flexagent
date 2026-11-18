import { describe, expect, it } from "vitest";
import {
  SETTINGS_BROWSER_ROUTE,
  SETTINGS_HUB_ROUTE,
  SETTINGS_NODE_ROUTE,
  SETTINGS_NODES_ROUTE,
  SETTINGS_ROOT,
  browserSettingsPath,
  hubSettingsPath,
  isNodeTabAvailable,
  isSettingsIndexPath,
  isSettingsPath,
  nodeSettingsPath,
  resolveDefaultNodeId,
  settingsHomePath,
  settingsPathForTarget,
  settingsScopeFromPath,
} from "./settings-routes.ts";

describe("settings-routes", () => {
  it("derives the route patterns and builders from the same bases", () => {
    expect(SETTINGS_ROOT).toBe("/settings");
    expect(SETTINGS_HUB_ROUTE).toBe("hub/:tab?");
    expect(SETTINGS_NODES_ROUTE).toBe("nodes");
    expect(SETTINGS_NODE_ROUTE).toBe("nodes/:node_id/:tab?");
    expect(SETTINGS_BROWSER_ROUTE).toBe("browser/:tab?");
    expect(hubSettingsPath("tokens")).toBe("/settings/hub/tokens");
    expect(nodeSettingsPath("agents", "node-1")).toBe("/settings/nodes/node-1/agents");
    expect(nodeSettingsPath("general")).toBe("/settings/nodes");
    expect(browserSettingsPath()).toBe("/settings/browser/appearance");
  });

  it("classifies settings pathnames", () => {
    expect(isSettingsPath("/settings")).toBe(true);
    expect(isSettingsPath("/settings/nodes/n1")).toBe(true);
    expect(isSettingsPath("/settings-other")).toBe(false);
    expect(isSettingsPath("/projects")).toBe(false);

    expect(settingsScopeFromPath("/settings/hub/tokens")).toBe("hub");
    expect(settingsScopeFromPath("/settings/nodes/n1/agents")).toBe("node");
    expect(settingsScopeFromPath("/settings/browser")).toBe("browser");
    expect(settingsScopeFromPath("/settings")).toBeNull();
    expect(settingsScopeFromPath("/projects")).toBeNull();
  });

  it("recognises the entry paths that redirect to a canonical tab", () => {
    expect(isSettingsIndexPath("/settings")).toBe(true);
    expect(isSettingsIndexPath("/settings/hub")).toBe(true);
    expect(isSettingsIndexPath("/settings/browser")).toBe(true);
    expect(isSettingsIndexPath("/settings/nodes")).toBe(true);
    expect(isSettingsIndexPath("/settings/nodes/n1")).toBe(true);
    expect(isSettingsIndexPath("/settings/nodes/n1/agents")).toBe(false);
    expect(isSettingsIndexPath("/settings/hub/tokens")).toBe(false);
  });

  it("offers every node tab regardless of the connection role", () => {
    expect(isNodeTabAvailable("process")).toBe(true);
    expect(isNodeTabAvailable("audit")).toBe(true);
    expect(isNodeTabAvailable("agents")).toBe(true);
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
