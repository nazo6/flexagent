import { useNavigate } from "@solidjs/router";
import { createEffect, createMemo } from "solid-js";
import { resolveDefaultNodeId, settingsHomePath } from "#lib/settings-routes.ts";
import { sync } from "#lib/stores/app.ts";

/** `/settings` entry point: replace with the home tab for the current role. */
export default function SettingsIndex() {
  const navigate = useNavigate();

  const isHub = () => sync.connection.role === "hub";
  const defaultNodeId = createMemo(() =>
    resolveDefaultNodeId(isHub(), sync.nodes, sync.connection.systemInfo?.node_id),
  );

  createEffect(
    () => settingsHomePath(isHub(), defaultNodeId()),
    (path) => {
      navigate(path, { replace: true });
    },
  );

  return null;
}
