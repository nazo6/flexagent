import { useNavigate, useParams } from "@solidjs/router";
import { createEffect, createMemo, Match, Show, Switch } from "solid-js";
import AgentManager from "#lib/components/settings/AgentManager.tsx";
import AuditLog from "#lib/components/settings/AuditLog.tsx";
import ClientTokenCard from "#lib/components/settings/ClientTokenCard.tsx";
import PairingPanel from "#lib/components/settings/PairingPanel.tsx";
import ProcessSettingsPanel from "#lib/components/settings/ProcessSettingsPanel.tsx";
import SettingsCatalog from "#lib/components/settings/SettingsCatalog.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { m } from "#lib/paraglide/messages.js";
import {
  isNodeTab,
  isNodeTabAvailable,
  nodeSettingsPath,
  resolveDefaultNodeId,
  type NodeSettingsTab,
} from "#lib/settings-routes.ts";
import { sync } from "#lib/stores/app.ts";

/** `/settings/nodes/:node_id/:tab?`: settings of one node (machine). */
export default function NodeSettings() {
  const params = useParams<{ node_id?: string; tab?: string }>();
  const navigate = useNavigate();

  const nodeId = () => params.node_id ?? "";
  const isHub = () => sync.connection.role === "hub";
  const defaultNodeId = createMemo(() =>
    resolveDefaultNodeId(isHub(), sync.nodes, sync.connection.systemInfo?.node_id),
  );

  const tab = createMemo<NodeSettingsTab>(() => {
    const raw = params.tab;
    return raw !== undefined && isNodeTab(raw) ? raw : "general";
  });
  const tabAvailable = createMemo(() => isNodeTabAvailable(tab()));

  // A tab-less node URL is an entry point: canonicalize it to `general`.
  createEffect(
    () => ({ node: nodeId(), tab: params.tab ?? null }),
    (target) => {
      if (target.tab === null && target.node !== "") {
        navigate(nodeSettingsPath("general", target.node), { replace: true });
      }
    },
  );

  // The node index (no node in the URL) picks the default node, as the Svelte
  // `/settings/nodes` page did.
  createEffect(
    () => ({ node: nodeId(), next: defaultNodeId() }),
    (target) => {
      if (target.node === "" && target.next !== null) {
        navigate(nodeSettingsPath("general", target.next), { replace: true });
      }
    },
  );

  return (
    <Show when={nodeId() !== "" && params.tab !== undefined}>
      <Switch>
        <Match when={!tabAvailable()}>
          <Card class="gap-3 p-5">
            <p class="text-muted-foreground text-[13px]">{m.settings_node_tab_local_only()}</p>
            <div>
              <Button href={nodeSettingsPath("general", nodeId())} variant="outline" size="sm">
                {m.settings_tab_general()}
              </Button>
            </div>
          </Card>
        </Match>

        <Match when={tab() === "general"}>
          <SettingsCatalog section="machine" nodeId={nodeId()} />
          <Show when={!isHub()}>
            <ClientTokenCard />
          </Show>
        </Match>

        <Match when={tab() === "process"}>
          <ProcessSettingsPanel nodeId={nodeId()} />
        </Match>

        <Match when={tab() === "pairing"}>
          <PairingPanel nodeId={nodeId()} />
        </Match>

        <Match when={tab() === "agents"}>
          <AgentManager nodeId={nodeId()} />
        </Match>

        <Match when={tab() === "audit"}>
          <AuditLog nodeId={nodeId()} />
        </Match>
      </Switch>
    </Show>
  );
}
