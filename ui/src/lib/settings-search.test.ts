import { describe, expect, it } from "vitest";
import { searchSettings } from "./settings-search.ts";

describe("searchSettings", () => {
  it("finds Browser Notifications and server VAPID entries", () => {
    const pushResults = searchSettings("push");
    expect(pushResults).toContainEqual(
      expect.objectContaining({
        scope: "browser",
        tab: "notifications",
        label: "Web Push notifications",
      }),
    );

    const notificationResults = searchSettings("notification");
    expect(notificationResults).toContainEqual(
      expect.objectContaining({
        scope: "browser",
        tab: "notifications",
      }),
    );

    const vapidResults = searchSettings("VAPID");
    expect(vapidResults).toContainEqual(
      expect.objectContaining({
        scope: "node",
        tab: "general",
        label: "VAPID subject",
      }),
    );
  });
});
