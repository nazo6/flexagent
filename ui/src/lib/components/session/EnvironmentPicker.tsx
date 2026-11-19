import { For, Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import PickerItem from "#lib/components/composer/PickerItem.tsx";
import type { NodeSummary, ProvisionerSummary } from "#lib/generated/bindings.ts";
import { getNodeAvailability } from "#lib/node-status.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { EnvironmentPillIcon } from "./new-session-icons.tsx";

export interface EnvironmentPickerProps {
  /** Value shown in the trigger. */
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  provisionerMode: boolean;
  nodes: readonly NodeSummary[];
  onlineNodeCount: number;
  selectedNodeId: string;
  onSelectNode: (nodeId: string) => void;
  provisioners: readonly ProvisionerSummary[];
  provisionerName: string;
  provisionerLoadError: string | null;
  onRetryProvisioners: () => void;
  onSelectProvisioner: (name: string) => void;
  /** Clone branch for the ephemeral VM. */
  branch: string;
  onBranchChange: (branch: string) => void;
  selectedProvisioner: ProvisionerSummary | null;
  testBusy: boolean;
  onTest: () => void;
  /** Availability dot color of the current node. */
  nodeAvailabilityDotClass: string;
}

/**
 * Environment pill: resident nodes and ephemeral-VM provisioners, plus the
 * clone branch and the connectivity test of the selected provisioner.
 */
export default function EnvironmentPicker(props: EnvironmentPickerProps) {
  const disabled = () => props.nodes.length === 0 && props.provisioners.length === 0;

  return (
    <PickerPill
      label={m.new_session_environment()}
      value={props.label}
      icon={
        <EnvironmentPillIcon
          provisioner={props.provisionerMode}
          dotColorClass={props.nodeAvailabilityDotClass}
        />
      }
      contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
      open={props.open}
      onOpenChange={(open) => props.onOpenChange(open)}
      disabled={disabled()}
    >
      <div class="max-h-80 overflow-y-auto p-1">
        <div class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase">
          {m.new_session_node_section()}
        </div>
        <Show
          when={props.onlineNodeCount === 0}
          fallback={
            <For each={props.nodes} keyed={(node) => node.node_id}>
              {(node) => {
                const availability = () => getNodeAvailability(node(), sync.wsConnected);
                return (
                  <PickerItem
                    selected={!props.provisionerMode && node().node_id === props.selectedNodeId}
                    disabled={!availability().isAvailable}
                    onSelect={() => props.onSelectNode(node().node_id)}
                  >
                    <span class="flex items-center gap-1.5">
                      <span
                        class={`size-1.5 shrink-0 rounded-full ${availability().dotColorClass}`}
                      />
                      <span class="truncate">{node().name}</span>
                    </span>
                    <span class="text-muted-foreground block truncate text-[11px]">
                      {availability().statusText}
                    </span>
                  </PickerItem>
                );
              }}
            </For>
          }
        >
          <p class="text-muted-foreground px-2 py-1.5 text-[13px]">
            {props.nodes.length === 0 ? m.new_session_no_nodes() : m.new_session_no_online_nodes()}
          </p>
        </Show>

        <div class="border-t p-1 pt-2">
          <div class="text-muted-foreground px-2 py-1 text-[11px] font-medium tracking-wide uppercase">
            {m.new_session_ephemeral_section()}
          </div>
          <Show
            when={props.provisioners.length > 0}
            fallback={
              <Show
                when={props.provisionerLoadError !== null}
                fallback={
                  <>
                    <p class="text-muted-foreground px-2 py-1 text-[11px]">
                      {m.new_session_provisioner_undefined()}
                    </p>
                    <p class="text-muted-foreground px-2 pb-1 text-[11px]">
                      {m.new_session_provisioner_config_hint()}
                    </p>
                  </>
                }
              >
                <div class="flex items-center gap-1.5 px-2 py-1 text-[11px]">
                  <span class="text-destructive truncate">{m.common_load_failed()}</span>
                  <button
                    type="button"
                    class="text-muted-foreground hover:text-foreground underline"
                    onClick={() => props.onRetryProvisioners()}
                  >
                    {m.common_retry()}
                  </button>
                </div>
              </Show>
            }
          >
            <For each={props.provisioners} keyed={(provisioner) => provisioner.name}>
              {(provisioner) => (
                <PickerItem
                  selected={props.provisionerMode && provisioner().name === props.provisionerName}
                  onSelect={() => props.onSelectProvisioner(provisioner().name)}
                >
                  <span class="block truncate">
                    {provisioner().description ?? provisioner().name}
                  </span>
                  <span class="text-muted-foreground block truncate text-[11px]">
                    {provisioner().name}
                  </span>
                </PickerItem>
              )}
            </For>
          </Show>

          <Show when={props.provisionerMode && props.selectedProvisioner !== null}>
            <div class="mt-2 flex flex-col gap-2 border-t px-2 pt-2">
              <div class="flex flex-col gap-1">
                <span class="text-muted-foreground text-[11px]">
                  {m.new_session_clone_branch()}
                </span>
                <Input
                  value={props.branch}
                  onValueChange={(value) => props.onBranchChange(value)}
                  placeholder={m.new_session_clone_branch_placeholder()}
                  class="h-8 text-[13px]"
                />
              </div>
              <div class="flex items-center justify-between gap-2">
                <span class="text-muted-foreground min-w-0 truncate text-[11px]">
                  {props.selectedProvisioner?.name ?? ""}
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 shrink-0 px-2 text-[13px]"
                  disabled={props.testBusy}
                  title={m.new_session_provisioner_test_button_title()}
                  onClick={() => props.onTest()}
                >
                  {m.new_session_test()}
                </Button>
              </div>
            </div>
          </Show>
        </div>
      </div>
    </PickerPill>
  );
}
