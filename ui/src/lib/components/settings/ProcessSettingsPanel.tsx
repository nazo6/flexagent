import { Show } from "solid-js";
import IconOctagonX from "~icons/lucide/octagon-x";
import KillSwitchButton from "#lib/components/KillSwitchButton.tsx";
import DiagnosticsPanel from "#lib/components/settings/DiagnosticsPanel.tsx";
import SettingsCatalog from "#lib/components/settings/SettingsCatalog.tsx";
import { Card } from "#lib/components/ui/card/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

/** Process tab: read-only bootstrap values, health and the emergency stop. */
export default function ProcessSettingsPanel(props: {
  /** Target node (`""` = the endpoint itself, i.e. the hub's own process). */
  nodeId?: string;
}) {
  // The kill switch is a hub-wide action; hide it under a specific node's tab.
  const showKillSwitch = () => !(sync.connection.role === "hub" && (props.nodeId ?? "") !== "");

  return (
    <>
      <SettingsCatalog section="bootstrap" nodeId={props.nodeId ?? ""} />
      <DiagnosticsPanel nodeId={props.nodeId ?? ""} />
      <Show when={showKillSwitch()}>
        <Card class="border-destructive/40 gap-3 p-4">
          <div class="flex flex-wrap items-start justify-between gap-3">
            <div class="flex items-start gap-2">
              <IconOctagonX class="text-destructive mt-0.5 size-4 shrink-0" />
              <div class="flex flex-col gap-1">
                <h2 class="font-medium">{m.settings_emergency_stop_title()}</h2>
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_emergency_stop_description()}
                </p>
              </div>
            </div>
            <KillSwitchButton />
          </div>
        </Card>
      </Show>
    </>
  );
}
