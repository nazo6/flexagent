import { createEffect, createMemo, createSignal, Show } from "solid-js";
import IconCopy from "~icons/lucide/copy";
import IconLink from "~icons/lucide/link";
import IconPlugZap from "~icons/lucide/plug-zap";
import IconUnplug from "~icons/lucide/unplug";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type { HubLinkState, NodeSettingsSnapshot } from "#lib/generated/bindings.ts";
import { hubLinkLabel } from "#lib/node-status.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

/**
 * Node to hub pairing panel.
 *
 * The settings page passes the selected node when connected to the hub;
 * defaults to the local node.
 */
export default function PairingPanel(props: { nodeId?: string }) {
  const targetNode = createMemo(() =>
    (props.nodeId ?? "") !== ""
      ? (props.nodeId ?? "")
      : (sync.connection.systemInfo?.node_id ?? ""),
  );

  const [snapshot, setSnapshot] = createSignal<NodeSettingsSnapshot | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  /** Target whose snapshot is currently loaded (null = none). */
  const [loadedTarget, setLoadedTarget] = createSignal<string | null>(null);
  const [hubUrl, setHubUrl] = createSignal("");
  const [token, setToken] = createSignal("");
  const [tested, setTested] = createSignal<HubLinkState | null>(null);

  const pairing = createMemo(() => snapshot()?.pairing ?? null);
  const hubConnection = createMemo(() => sync.hubLink ?? pairing()?.connection ?? null);
  const linkState = createMemo<HubLinkState>(() => hubConnection()?.state ?? "standalone");
  const switchesHub = createMemo(
    () =>
      hubUrl().trim() !== "" &&
      pairing()?.hub_url !== null &&
      pairing()?.hub_url !== hubUrl().trim(),
  );

  let generation = 0;
  /** Plain mirror of `loadedTarget`, readable from an effect apply. */
  let loadedTargetValue: string | null = null;

  createEffect(
    () => targetNode(),
    (node) => {
      // Load once the node identity is known. Do not track `snapshot` here:
      // `load()` assigns it, which would retrigger the effect and overwrite the
      // form while the user types. Save/clear already refresh the snapshot
      // from the response.
      if (node === "") return;
      if (loadedTargetValue !== node) {
        setSnapshot(null);
        setLoadedTarget(null);
        loadedTargetValue = null;
        setHubUrl("");
        setToken("");
        setTested(null);
      }
      void load(node);
      return () => {
        // A response that lands after a node switch / unmount is discarded.
        generation += 1;
      };
    },
  );

  async function load(node = targetNode()): Promise<void> {
    if (node === "") return;
    const current = ++generation;
    setError(null);
    try {
      const next = await sync.connection.client.nodeSettings(node);
      if (current !== generation) return;
      setSnapshot(next);
      loadedTargetValue = node;
      setLoadedTarget(node);
      setHubUrl(next.pairing.hub_url ?? "");
      setToken("");
    } catch (cause) {
      if (current !== generation) return;
      setError(errorMessage(cause));
    }
  }

  function linkStateLabel(value: HubLinkState): string {
    return hubLinkLabel(value);
  }

  function linkVariant(value: HubLinkState): "secondary" | "outline" | "destructive" {
    if (value === "connected") return "secondary";
    if (value === "standalone") return "outline";
    return "destructive";
  }

  async function test(): Promise<void> {
    const current = snapshot();
    if (busy() || hubUrl().trim() === "") return;
    if (loadedTarget() !== targetNode() || current === null) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    setBusy(true);
    setTested(null);
    try {
      const status = await sync.connection.client.testPairing(targetNode(), {
        hub_url: hubUrl().trim(),
        token: token().trim() === "" ? null : token().trim(),
      });
      setTested(status.state);
      if (status.state === "connected") {
        toast.success(m.settings_pairing_test_ok());
      } else {
        toast.error(status.message ?? linkStateLabel(status.state));
      }
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function save(): Promise<void> {
    const current = snapshot();
    if (busy() || hubUrl().trim() === "") return;
    if (loadedTarget() !== targetNode() || current === null) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    setBusy(true);
    try {
      const next = await sync.connection.client.updatePairing(targetNode(), {
        hub_url: hubUrl().trim(),
        token: token().trim() === "" ? null : token().trim(),
        expected_revision: current.pairing.revision,
      });
      setSnapshot(next);
      setToken("");
      setTested(null);
      toast.success(m.settings_pairing_saved({ url: next.pairing.hub_url ?? "" }));
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function clear(): Promise<void> {
    const current = snapshot();
    if (busy()) return;
    if (loadedTarget() !== targetNode() || current === null) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    setBusy(true);
    try {
      setSnapshot(
        await sync.connection.client.clearPairing(targetNode(), {
          expected_revision: current.pairing.revision,
        }),
      );
      setHubUrl("");
      setToken("");
      setTested(null);
      toast.success(m.settings_pairing_cleared());
    } catch (cause) {
      toast.error(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copyNodeId(): Promise<void> {
    try {
      await navigator.clipboard.writeText(targetNode());
      toast.success(m.settings_copied({ name: m.settings_pairing_node_id() }));
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }

  return (
    <Card class="gap-3 p-4">
      <div class="flex items-start gap-2">
        <IconPlugZap class="text-muted-foreground mt-0.5 size-4 shrink-0" />
        <div class="flex flex-1 flex-col gap-3">
          <div class="flex flex-col gap-1">
            <h2 class="font-medium">{m.settings_pairing_title()}</h2>
            <p class="text-muted-foreground text-[13px]">{m.settings_pairing_description()}</p>
          </div>

          <div class="flex flex-wrap items-center gap-2 text-[13px]">
            <span class="text-muted-foreground">{m.settings_pairing_state()}</span>
            <Badge variant={linkVariant(linkState())} title={hubConnection()?.message ?? undefined}>
              {linkStateLabel(linkState())}
            </Badge>
            <Show when={hubConnection()?.hub_url}>
              <code class="text-muted-foreground text-[11px]">{hubConnection()?.hub_url}</code>
            </Show>
          </div>

          <div class="flex flex-wrap items-center gap-2 text-[13px]">
            <span class="text-muted-foreground">{m.settings_pairing_node_id()}</span>
            <code class="text-[12px]">{targetNode()}</code>
            <Button variant="ghost" size="sm" onClick={() => void copyNodeId()}>
              <IconCopy />
              {m.settings_pairing_copy_node_id()}
            </Button>
          </div>

          <div class="grid gap-2 sm:grid-cols-2">
            <div class="flex flex-col gap-1">
              <Label for="pairing-hub-url">{m.settings_pairing_hub_url()}</Label>
              <Input
                id="pairing-hub-url"
                value={hubUrl()}
                onValueChange={setHubUrl}
                placeholder="http://hub.example:8080"
                autocomplete="off"
                spellcheck="false"
              />
            </div>
            <div class="flex flex-col gap-1">
              <Label for="pairing-token">{m.settings_pairing_token()}</Label>
              <Input
                id="pairing-token"
                type="password"
                value={token()}
                onValueChange={setToken}
                placeholder={
                  pairing()?.token_configured
                    ? m.settings_pairing_token_configured()
                    : m.settings_pairing_token_placeholder()
                }
                autocomplete="off"
                spellcheck="false"
              />
            </div>
          </div>

          <p class="text-muted-foreground text-[12px]">{m.settings_pairing_token_hint()}</p>

          <Show when={switchesHub()}>
            <p class="text-[12px] text-amber-600 dark:text-amber-400">
              {m.settings_pairing_switch_warning()}
            </p>
          </Show>

          <Show when={pairing()?.sessions_truncated}>
            <p class="text-[12px] text-amber-600 dark:text-amber-400">
              {m.settings_pairing_truncated()}
            </p>
          </Show>

          <Show when={error()}>
            <p class="text-destructive text-[13px]">{error()}</p>
          </Show>

          <div class="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={
                busy() ||
                hubUrl().trim() === "" ||
                snapshot() === null ||
                loadedTarget() !== targetNode()
              }
              onClick={() => void test()}
            >
              <IconLink />
              {m.settings_pairing_test()}
            </Button>
            <Button
              size="sm"
              disabled={
                busy() ||
                hubUrl().trim() === "" ||
                snapshot() === null ||
                loadedTarget() !== targetNode()
              }
              onClick={() => void save()}
            >
              {m.settings_pairing_save()}
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={
                busy() ||
                pairing()?.hub_url === null ||
                snapshot() === null ||
                loadedTarget() !== targetNode()
              }
              onClick={() => void clear()}
            >
              <IconUnplug />
              {m.settings_pairing_clear()}
            </Button>
            <Show when={tested() === "connected"}>
              <span class="text-muted-foreground text-[12px]">{m.settings_pairing_test_ok()}</span>
            </Show>
          </div>
        </div>
      </div>
    </Card>
  );
}
