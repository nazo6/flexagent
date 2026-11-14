<script lang="ts">
  import { untrack } from "svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type {
    ElicitationAction,
    ElicitationProperty,
    ElicitationSchema,
    ElicitationValue,
  } from "#lib/generated/bindings.ts";
  import { formatRelativeTime } from "#lib/format.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import CircleHelpIcon from "@lucide/svelte/icons/circle-help";
  import SendIcon from "@lucide/svelte/icons/send";
  import XIcon from "@lucide/svelte/icons/x";

  type FormValue = string | number | boolean | string[];

  let {
    sessionId,
    elicitationId,
    message,
    requestedSchema,
    resolved = null,
    createdAt = null,
  }: {
    sessionId: string;
    elicitationId: string;
    message: string;
    requestedSchema: ElicitationSchema;
    resolved?: { action: ElicitationAction; resolvedBy: string } | null;
    createdAt?: number | null;
  } = $props();

  const properties = $derived(Object.entries(requestedSchema.properties ?? {}));
  const required = $derived(new Set(requestedSchema.required ?? []));

  /** Seed the form from the schema (default / empty) for this elicitation. */
  function initialValues(schema: ElicitationSchema): Record<string, FormValue> {
    const next: Record<string, FormValue> = {};
    for (const [key, property] of Object.entries(schema.properties ?? {})) {
      if (property.default !== undefined) {
        next[key] = property.default as FormValue;
      } else if (property.type === "boolean") {
        next[key] = false;
      } else if (property.type === "array") {
        next[key] = [];
      } else {
        next[key] = "";
      }
    }
    return next;
  }

  // A card is created per elicitation, so the schema is fixed for its lifetime.
  let values = $state<Record<string, FormValue>>(untrack(() => initialValues(requestedSchema)));
  let busyAction = $state<ElicitationAction | null>(null);

  const missingRequired = $derived(
    [...required].filter((key) => {
      const value = values[key];
      if (value === undefined) return true;
      if (typeof value === "string") return value.trim() === "";
      if (Array.isArray(value)) return value.length === 0;
      return false;
    }),
  );

  const resolvedLabel: Record<ElicitationAction, string> = {
    accept: m.chat_elicitation_accepted(),
    decline: m.chat_elicitation_declined(),
    cancel: m.chat_elicitation_cancelled(),
  };

  function enumOptions(property: ElicitationProperty): { value: string; label: string }[] {
    if (property.enum && property.enum.length > 0) {
      return property.enum.map((value) => ({ value, label: value }));
    }
    return (property.oneOf ?? []).map((option) => ({
      value: option.const,
      label: option.title ?? option.const,
    }));
  }

  function setString(key: string, value: string): void {
    values[key] = value;
  }

  function setNumber(key: string, raw: string): void {
    values[key] = raw === "" ? "" : Number(raw);
  }

  function setStringArray(key: string, raw: string): void {
    values[key] = raw
      .split(",")
      .map((part) => part.trim())
      .filter((part) => part !== "");
  }

  function toggleArrayValue(key: string, option: string, checked: boolean): void {
    const current = values[key];
    const list = Array.isArray(current) ? current : [];
    values[key] = checked ? [...list, option] : list.filter((value) => value !== option);
  }

  /** Content sent with `accept` (empty optional fields are omitted). */
  function buildContent(): Record<string, ElicitationValue> {
    const content: Record<string, ElicitationValue> = {};
    for (const [key] of properties) {
      const value = values[key];
      if (typeof value === "string") {
        if (value.trim() !== "") content[key] = value;
      } else if (Array.isArray(value)) {
        if (value.length > 0) content[key] = value;
      } else if (value !== undefined) {
        content[key] = value;
      }
    }
    return content;
  }

  async function respond(action: ElicitationAction) {
    if (busyAction !== null) return;
    if (action === "accept" && missingRequired.length > 0) {
      toast.error(m.chat_required_fields({ fields: missingRequired.join(", ") }));
      return;
    }
    busyAction = action;
    try {
      const result = await sync.respondElicitation(
        sessionId,
        elicitationId,
        action,
        action === "accept" ? buildContent() : null,
      );
      if (result.already_resolved) {
        toast.info(m.error_already_resolved());
      } else {
        toast.success(m.chat_elicitation_sent());
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busyAction = null;
    }
  }
</script>

<div class="bg-card border-border flex flex-col gap-3 rounded-lg border p-3">
  <div class="flex min-w-0 flex-col gap-1">
    <div class="flex items-center gap-2">
      <CircleHelpIcon class="text-signal size-4 shrink-0" />
      <span class="text-[13px] font-medium">{m.chat_elicitation_heading()}</span>
      {#if resolved}
        <Badge variant="secondary">
          {resolvedLabel[resolved.action]} ({resolved.resolvedBy})
        </Badge>
      {:else if createdAt !== null}
        <span class="text-muted-foreground text-[13px]">{formatRelativeTime(createdAt)}</span>
      {/if}
    </div>
    <p class="text-sm break-words">{message}</p>
  </div>

  {#if resolved === null}
    <div class="flex flex-col gap-3">
      {#each properties as [key, property] (key)}
        {@const inputId = `elicitation-${elicitationId}-${key}`}
        <div class="flex flex-col gap-1.5">
          <Label for={inputId}>
            {property.title ?? key}
            {#if required.has(key)}<span class="text-destructive">*</span>{/if}
          </Label>
          {#if property.description}
            <p class="text-muted-foreground text-[13px]">{property.description}</p>
          {/if}
          {#if property.enum || property.oneOf}
            <select
              id={inputId}
              class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
              value={typeof values[key] === "string" ? values[key] : ""}
              onchange={(event) => setString(key, event.currentTarget.value)}
            >
              <option value="">{m.chat_select_option()}</option>
              {#each enumOptions(property) as option (option.value)}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          {:else if property.type === "boolean"}
            <label class="flex items-center gap-2 text-sm">
              <input
                id={inputId}
                type="checkbox"
                class="size-4"
                checked={values[key] === true}
                onchange={(event) => (values[key] = event.currentTarget.checked)}
              />
              <span>{m.chat_yes()}</span>
            </label>
          {:else if property.type === "array"}
            {#if property.items?.enum}
              <div class="flex flex-col gap-1.5">
                {#each property.items.enum as option (option)}
                  <label class="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      class="size-4"
                      checked={Array.isArray(values[key]) && values[key].includes(option)}
                      onchange={(event) =>
                        toggleArrayValue(key, option, event.currentTarget.checked)}
                    />
                    <span>{option}</span>
                  </label>
                {/each}
              </div>
            {:else}
              <Input
                id={inputId}
                placeholder={m.chat_comma_separated()}
                value={Array.isArray(values[key]) ? values[key].join(", ") : ""}
                oninput={(event) => setStringArray(key, event.currentTarget.value)}
              />
            {/if}
          {:else if property.type === "integer" || property.type === "number"}
            <Input
              id={inputId}
              type="number"
              value={typeof values[key] === "number" ? String(values[key]) : ""}
              oninput={(event) => setNumber(key, event.currentTarget.value)}
            />
          {:else}
            <Input
              id={inputId}
              value={typeof values[key] === "string" ? values[key] : ""}
              oninput={(event) => setString(key, event.currentTarget.value)}
            />
          {/if}
        </div>
      {/each}

      <div class="flex flex-wrap gap-2">
        <Button
          size="sm"
          disabled={busyAction !== null || missingRequired.length > 0}
          onclick={() => void respond("accept")}
        >
          <SendIcon />
          {m.chat_respond()}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busyAction !== null}
          onclick={() => void respond("decline")}
        >
          <XIcon />
          {m.chat_decline()}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busyAction !== null}
          onclick={() => void respond("cancel")}
        >
          {m.common_cancel()}
        </Button>
      </div>
    </div>
  {/if}
</div>
