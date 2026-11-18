import { useNavigate, useParams } from "@solidjs/router";
import { createEffect, createMemo, Match, Show, Switch } from "solid-js";
import AuditLog from "#lib/components/settings/AuditLog.tsx";
import ClientTokenCard from "#lib/components/settings/ClientTokenCard.tsx";
import NodeTokensPanel from "#lib/components/settings/NodeTokensPanel.tsx";
import ProcessSettingsPanel from "#lib/components/settings/ProcessSettingsPanel.tsx";
import ProvisionerManager from "#lib/components/settings/ProvisionerManager.tsx";
import SettingsCatalog from "#lib/components/settings/SettingsCatalog.tsx";
import { hubSettingsPath, isHubTab, type HubSettingsTab } from "#lib/settings-routes.ts";

/** `/settings/hub/:tab?`: hub (server) settings. */
export default function HubSettings() {
  const params = useParams<{ tab?: string }>();
  const navigate = useNavigate();

  const tab = createMemo<HubSettingsTab>(() => {
    const raw = params.tab;
    return raw !== undefined && isHubTab(raw) ? raw : "general";
  });

  createEffect(
    () => params.tab ?? null,
    (raw) => {
      if (raw === null) navigate(hubSettingsPath("general"), { replace: true });
    },
  );

  return (
    <Show when={params.tab !== undefined}>
      <Switch>
        <Match when={tab() === "general"}>
          <SettingsCatalog section="hub" />
          <ClientTokenCard />
        </Match>
        <Match when={tab() === "process"}>
          <ProcessSettingsPanel />
        </Match>
        <Match when={tab() === "tokens"}>
          <NodeTokensPanel />
        </Match>
        <Match when={tab() === "provisioners"}>
          <ProvisionerManager />
        </Match>
        <Match when={tab() === "audit"}>
          <AuditLog />
        </Match>
      </Switch>
    </Show>
  );
}
