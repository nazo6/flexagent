import { For, Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import PickerPill from "#lib/components/composer/PickerPill.tsx";
import ConfigOptionPicker from "#lib/components/composer/ConfigOptionPicker.tsx";
import type { ConfigOptionInfo, ConfigOptionValue } from "#lib/generated/bindings.ts";
import { isModelOption } from "#lib/config-options.ts";
import { m } from "#lib/paraglide/messages.js";
import { ModelPillIcon } from "./new-session-icons.tsx";

export interface ModelSelectorsProps {
  /** A fork reuses the parent's configuration: models are chosen after start. */
  fork: boolean;
  provisionerMode: boolean;
  /** The probe result belongs to the current node / agent / path. */
  probeReady: boolean;
  loading: boolean;
  /** A probe result for the current target is available. */
  capabilitiesReady: boolean;
  capabilitiesError: string | null;
  options: readonly ConfigOptionInfo[];
  values: Record<string, ConfigOptionValue>;
  favoriteScope: string;
  onChange: (key: string, value: ConfigOptionValue) => void;
  onRefresh: () => void;
}

/** Width of the model tab: a full-width dropdown does not fit the pill row. */
const PILL = "shrink-0";

/**
 * Model / config-option pills below the composer.
 *
 * One of five mutually exclusive states decides what is shown: after-start
 * (fork), loading, the option list, probe failure (with retry), an empty list,
 * and "not probed yet" (disabled).
 */
export default function ModelSelectors(props: ModelSelectorsProps) {
  const pending = () => props.probeReady && props.loading;
  return (
    <>
      <Show when={props.fork}>
        <PickerPill
          label={m.new_session_model()}
          value={m.new_session_models_after_start()}
          icon={<ModelPillIcon />}
          class={PILL}
          disabled={true}
        />
      </Show>
      <Show when={!props.fork && !props.provisionerMode && pending()}>
        <PickerPill
          label={m.new_session_model()}
          value={m.new_session_models_loading()}
          icon={<ModelPillIcon />}
          class={PILL}
          loading={true}
          disabled={true}
        />
      </Show>
      <Show when={!props.fork && !props.provisionerMode && !pending() && props.options.length > 0}>
        <For each={props.options} keyed={(option) => option.key}>
          {(option) => (
            <ConfigOptionPicker
              option={option()}
              value={props.values[option().key]}
              class={PILL}
              favoriteScope={props.favoriteScope}
              onChange={(value) => props.onChange(option().key, value)}
              showRefresh={isModelOption(option())}
              onRefresh={() => props.onRefresh()}
            />
          )}
        </For>
      </Show>
      <Show
        when={
          !props.fork &&
          !props.provisionerMode &&
          !pending() &&
          props.options.length === 0 &&
          props.capabilitiesError !== null
        }
      >
        <PickerPill
          label={m.new_session_model()}
          value={m.new_session_model_default()}
          icon={<ModelPillIcon />}
          class={PILL}
          contentClass="w-72 p-0"
        >
          <div class="flex flex-col gap-2 p-2 text-[13px]">
            <p class="text-muted-foreground">{m.new_session_models_unavailable()}</p>
            <p class="text-destructive text-[11px] break-words">{props.capabilitiesError}</p>
            <Button
              variant="outline"
              size="sm"
              class="self-start"
              onClick={() => props.onRefresh()}
            >
              {m.common_retry()}
            </Button>
          </div>
        </PickerPill>
      </Show>
      <Show
        when={
          !props.fork &&
          !props.provisionerMode &&
          !pending() &&
          props.options.length === 0 &&
          props.capabilitiesError === null &&
          props.capabilitiesReady &&
          props.probeReady
        }
      >
        <PickerPill
          label={m.new_session_model()}
          value={m.new_session_model_default()}
          icon={<ModelPillIcon />}
          class={PILL}
          contentClass="w-72 p-2"
        >
          <p class="text-muted-foreground text-[13px]">{m.new_session_models_empty()}</p>
        </PickerPill>
      </Show>
      <Show
        when={
          !props.fork &&
          !props.provisionerMode &&
          !pending() &&
          props.options.length === 0 &&
          props.capabilitiesError === null &&
          !(props.capabilitiesReady && props.probeReady)
        }
      >
        <PickerPill
          label={m.new_session_model()}
          value={m.new_session_model_default()}
          icon={<ModelPillIcon />}
          class={PILL}
          disabled={true}
        />
      </Show>
    </>
  );
}
