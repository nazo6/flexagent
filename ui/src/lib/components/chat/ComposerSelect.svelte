<script lang="ts">
  import * as Select from "#lib/components/ui/select/index.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";

  interface Item {
    /** Select value (the value sent to the agent) */
    value: string;
    /** Label shown in the trigger and the list */
    label: string;
    /** Optional supplementary description shown in the list */
    description?: string | null;
  }

  interface Props {
    /** Fixed label describing what is selected (for example mode / model) */
    label: string;
    /** Current value (undefined when it is not among the choices) */
    value?: string;
    /** Display when nothing is selected */
    placeholder?: string;
    items: Item[];
    disabled?: boolean;
    class?: string;
    onchange: (value: string) => void;
  }

  let {
    label,
    value = undefined,
    placeholder = m.chat_not_selected(),
    items,
    disabled = false,
    class: className = "",
    onchange,
  }: Props = $props();
</script>

<!--
  Settings selector below the chat input (mode / model, etc.).
  - The trigger always shows the item name so it is clear what is being selected
  - `items` is passed to Root so the trigger and list labels always use the same
    string (this prevents raw values such as mode_id / JSON / "null" from showing
    before mount)
-->
<Select.Root
  type="single"
  {items}
  {disabled}
  {value}
  onValueChange={(next) => {
    if (typeof next === "string" && next !== "") onchange(next);
  }}
>
  <Select.Trigger size="sm" class={cn("max-w-64", className)}>
    <span class="text-muted-foreground shrink-0 text-xs">{label}</span>
    <Select.Value {placeholder} />
  </Select.Trigger>
  <Select.Content class="min-w-48">
    {#each items as item (item.value)}
      <Select.Item value={item.value} label={item.label}>
        <span class="flex min-w-0 flex-col">
          <span class="truncate">{item.label}</span>
          {#if item.description}
            <span class="text-muted-foreground max-w-72 truncate text-xs">{item.description}</span>
          {/if}
        </span>
      </Select.Item>
    {/each}
  </Select.Content>
</Select.Root>
