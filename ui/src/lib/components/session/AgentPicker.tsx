import { For, Show } from "solid-js";
import AgentIcon from "#lib/components/agent/AgentIcon.tsx";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import PickerItem from "#lib/components/composer/PickerItem.tsx";
import { agentHint } from "#lib/agents.ts";
import type { AgentSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { AgentPillIcon } from "./new-session-icons.tsx";
import IconLoaderCircle from "~icons/lucide/loader-circle";

export interface AgentPickerProps {
  /** Value shown in the trigger. */
  label: string;
  /** The agent in effect (its icon decorates the trigger). */
  selectedIcon: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** A fork inherits the parent's agent, so the choice is locked. */
  locked: boolean;
  /** Show the free-form agent ID input instead of the catalog list. */
  manual: boolean;
  agentId: string;
  onAgentIdChange: (agentId: string) => void;
  /** Typing an ID counts as a manual choice (persisted with the typed value). */
  onManualInput: (event: InputEvent & { currentTarget: HTMLInputElement }) => void;
  catalogLoading: boolean;
  catalogError: string | null;
  agents: readonly AgentSummary[];
  effectiveAgentId: string;
  onSelect: (agentId: string) => void;
  onRetry: () => void;
  onToggleManual: () => void;
  agentsSettingsHref: string;
}

/**
 * Agent pill: the node's catalog, a free-form ID and a link to agent
 * settings. A fork session keeps the parent agent, so the picker is read-only.
 */
export default function AgentPicker(props: AgentPickerProps) {
  return (
    <PickerPill
      label={m.new_session_agent()}
      value={props.label}
      icon={<AgentPillIcon icon={props.selectedIcon} />}
      class="shrink-0"
      contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
      open={props.open}
      onOpenChange={(open) => props.onOpenChange(open)}
      disabled={props.locked}
    >
      <Show
        when={props.manual}
        fallback={
          <Show
            when={props.catalogLoading}
            fallback={
              <Show
                when={props.catalogError !== null}
                fallback={
                  <Show
                    when={props.agents.length === 0}
                    fallback={
                      <div class="max-h-72 overflow-y-auto p-1">
                        <For each={props.agents} keyed={(agent) => agent.id}>
                          {(agent) => (
                            <PickerItem
                              selected={agent().id === props.effectiveAgentId}
                              onSelect={() => props.onSelect(agent().id)}
                            >
                              <span class="flex min-w-0 items-center gap-2">
                                <AgentIcon icon={agent().icon} class="size-4" />
                                <span class="min-w-0 flex-1">
                                  <span class="block truncate">{agent().name}</span>
                                  <span class="text-muted-foreground block truncate font-mono text-[11px]">
                                    {agent().id} — {agentHint(agent())}
                                  </span>
                                </span>
                              </span>
                            </PickerItem>
                          )}
                        </For>
                      </div>
                    }
                  >
                    <p class="text-muted-foreground p-2 text-[13px]">{m.new_session_no_agents()}</p>
                  </Show>
                }
              >
                <div class="flex flex-col gap-2 p-2">
                  <p class="text-destructive text-[11px] break-words">{props.catalogError}</p>
                  <Button
                    variant="outline"
                    size="sm"
                    class="self-start"
                    onClick={() => props.onRetry()}
                  >
                    {m.common_retry()}
                  </Button>
                </div>
              </Show>
            }
          >
            <p class="text-muted-foreground flex items-center gap-1.5 px-2 py-2 text-[13px]">
              <IconLoaderCircle class="size-3.5 animate-spin" />
              {m.common_loading()}
            </p>
          </Show>
        }
      >
        <div class="flex flex-col gap-2 p-2">
          <Input
            value={props.agentId}
            onValueChange={(value) => props.onAgentIdChange(value)}
            onInput={(event) => props.onManualInput(event)}
            placeholder={m.new_session_agent_id_placeholder()}
            class="h-8 font-mono text-[13px]"
            disabled={props.locked}
          />
          <p class="text-muted-foreground text-[11px]">{m.new_session_agent_choose_hint()}</p>
        </div>
      </Show>
      <div class="border-t p-1">
        <button
          type="button"
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block w-full rounded-md px-2 py-1.5 text-left text-[13px] disabled:pointer-events-none disabled:opacity-50"
          disabled={props.locked}
          onClick={() => props.onToggleManual()}
        >
          {props.manual ? m.new_session_choose_from_list() : m.new_session_enter_id()}
        </button>
        <a
          href={props.agentsSettingsHref}
          class="text-muted-foreground hover:bg-accent hover:text-accent-foreground block rounded-md px-2 py-1.5 text-[13px]"
        >
          {m.new_session_manage_agents()}
        </a>
      </div>
    </PickerPill>
  );
}
