import { createMemo, createSignal, For, Show } from "solid-js";
import PickerItem from "./PickerItem.tsx";
import PickerPill from "./PickerPill.tsx";
import type { ModeInfo } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import IconSlidersHorizontal from "~icons/lucide/sliders-horizontal";

export interface ModePickerProps {
  /** Modes reported by the agent (empty while the probe has not answered). */
  modes: ModeInfo[];
  /** Current mode ID (`null` = unspecified). */
  value?: string | null;
  /** Offer an explicit "default" choice (new sessions send `null`). */
  allowDefault?: boolean;
  loading?: boolean;
  disabled?: boolean;
  onchange: (modeId: string | null) => void;
  class?: string;
}

/** Mode selector (agent-specific modes such as `build` / `plan`). */
export default function ModePicker(props: ModePickerProps) {
  const items = createMemo(() =>
    props.modes.map((mode) => ({
      value: mode.mode_id,
      label: mode.name.trim() === "" ? mode.mode_id : mode.name,
      description: mode.description,
    })),
  );

  const [open, setOpen] = createSignal(false);

  const selected = createMemo(() => items().find((item) => item.value === props.value) ?? null);

  /** Show the ID when the agent no longer reports the selected mode. */
  const currentLabel = () =>
    selected()?.label ??
    ((props.value ?? null) === null
      ? props.allowDefault
        ? m.new_session_mode_default_value()
        : ""
      : (props.value ?? ""));

  return (
    <PickerPill
      label={m.chat_mode_label()}
      value={currentLabel()}
      icon={<IconSlidersHorizontal class="text-muted-foreground size-3.5" />}
      loading={props.loading ?? false}
      disabled={(props.disabled ?? false) || (props.loading ?? false)}
      class={props.class}
      open={open()}
      onOpenChange={setOpen}
      contentClass="w-[min(20rem,calc(100vw-1.5rem))]"
    >
      <div class="max-h-72 overflow-y-auto p-1">
        <Show when={props.allowDefault ?? false}>
          <PickerItem
            selected={(props.value ?? null) === null}
            onselect={() => {
              props.onchange(null);
              setOpen(false);
            }}
          >
            <span class="block truncate">{m.new_session_mode_default()}</span>
          </PickerItem>
        </Show>
        <For each={items()}>
          {(item) => (
            <PickerItem
              selected={item.value === props.value}
              onselect={() => {
                props.onchange(item.value);
                setOpen(false);
              }}
            >
              <span class="block truncate">{item.label}</span>
              <Show when={item.description}>
                {(description) => (
                  <span class="text-muted-foreground block truncate text-[11px]">
                    {description()}
                  </span>
                )}
              </Show>
            </PickerItem>
          )}
        </For>
      </div>
    </PickerPill>
  );
}
