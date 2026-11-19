import { createMemo, createSignal, For, Show } from "solid-js";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import type {
  ElicitationAction,
  ElicitationProperty,
  ElicitationSchema,
  ElicitationValue,
} from "#lib/generated/bindings.ts";
import { formatRelativeTime } from "#lib/format.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { createStore } from "solid-js";
import { untrack } from "solid-js";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconCircleHelp from "~icons/lucide/circle-help";
import IconSend from "~icons/lucide/send";
import IconX from "~icons/lucide/x";

type FormValue = string | number | boolean | string[];

export interface ElicitationCardProps {
  sessionId: string;
  elicitationId: string;
  message: string;
  requestedSchema: ElicitationSchema;
  resolved?: {
    action: ElicitationAction;
    resolvedBy: string;
    content: Record<string, ElicitationValue> | null;
  } | null;
  createdAt?: number | null;
}

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

/** Seed the free-text drafts of the fields that need a string while editing. */
function initialDrafts(schema: ElicitationSchema): Record<string, string> {
  const drafts: Record<string, string> = {};
  for (const [key, property] of Object.entries(schema.properties ?? {})) {
    const value = property.default as FormValue | undefined;
    if (property.type === "integer" || property.type === "number") {
      drafts[key] = typeof value === "number" ? String(value) : "";
    } else if (property.type === "array" && !property.items?.enum) {
      drafts[key] = Array.isArray(value) ? value.join(", ") : "";
    }
  }
  return drafts;
}

/**
 * Structured question card for an `elicitation/request`.
 *
 * The schema is fixed for the lifetime of one card, so the form is seeded once
 * when the card is created.
 */
export default function ElicitationCard(props: ElicitationCardProps) {
  const elicitationId = createMemo(() => props.elicitationId);
  const allProperties = createMemo(() => Object.entries(props.requestedSchema.properties ?? {}));
  // OpenCode form fields can be hidden while still carrying a value/default.
  const properties = createMemo(() =>
    allProperties().filter(([, property]) => property.hidden !== true),
  );
  const required = createMemo(() => new Set(props.requestedSchema.required ?? []));

  const [values, setValues] = createStore<Record<string, FormValue>>(
    untrack(() => initialValues(props.requestedSchema)),
  );
  /**
   * Text shown by the numeric / comma-separated array inputs.
   *
   * The committed `values` only receive a parseable number or a finished list,
   * so typing `-`, `1.` or a trailing comma stays possible.
   */
  const [drafts, setDrafts] = createStore<Record<string, string>>(
    untrack(() => initialDrafts(props.requestedSchema)),
  );
  const [busyAction, setBusyAction] = createSignal<ElicitationAction | null>(null);

  const missingRequired = createMemo(() =>
    [...required()].filter((key) => {
      const value = values[key];
      if (value === undefined) return true;
      if (typeof value === "string") return value.trim() === "";
      if (Array.isArray(value)) return value.length === 0;
      return false;
    }),
  );

  const resolvedLabel: Record<ElicitationAction, () => string> = {
    accept: () => m.chat_elicitation_accepted(),
    decline: () => m.chat_elicitation_declined(),
    cancel: () => m.chat_elicitation_cancelled(),
  };

  function optionLabel(property: ElicitationProperty | undefined, value: string): string {
    return (property?.oneOf ?? []).find((option) => option.const === value)?.title ?? value;
  }

  function formatValue(property: ElicitationProperty | undefined, value: ElicitationValue): string {
    if (typeof value === "string") return optionLabel(property, value);
    if (typeof value === "boolean") return value ? m.chat_yes() : m.chat_no();
    if (typeof value === "number") return String(value);
    if (Array.isArray(value))
      return value
        .map((item) => (typeof item === "string" ? optionLabel(property, item) : String(item)))
        .join(", ");
    return JSON.stringify(value);
  }

  /** Answers to show after `accept` (empty while the question is open). */
  const answerRows = createMemo(() => {
    const resolved = props.resolved ?? null;
    if (resolved === null || resolved.action !== "accept" || resolved.content === null) return [];
    const schemaProperties = props.requestedSchema.properties ?? {};
    return Object.entries(resolved.content).map(([key, value]) => ({
      key,
      label: schemaProperties[key]?.title ?? key,
      value: formatValue(schemaProperties[key], value),
    }));
  });

  function enumOptions(property: ElicitationProperty): { value: string; label: string }[] {
    if (property.enum && property.enum.length > 0) {
      return property.enum.map((value) => ({ value, label: value }));
    }
    return (property.oneOf ?? []).map((option) => ({
      value: option.const,
      label: option.title ?? option.const,
    }));
  }

  function setField(key: string, value: FormValue): void {
    setValues((current) => {
      current[key] = value;
    });
  }

  function setDraft(key: string, raw: string): void {
    setDrafts((current) => {
      current[key] = raw;
    });
  }

  /** Commits a number only once the text parses; keeps intermediate input. */
  function onNumberInput(key: string, raw: string): void {
    setDraft(key, raw);
    const trimmed = raw.trim();
    if (trimmed === "") {
      setField(key, "");
      return;
    }
    const parsed = Number(trimmed);
    if (Number.isFinite(parsed)) setField(key, parsed);
  }

  function commitNumber(key: string): void {
    const raw = drafts[key] ?? "";
    const trimmed = raw.trim();
    if (trimmed === "") {
      setField(key, "");
      return;
    }
    const parsed = Number(trimmed);
    if (Number.isFinite(parsed)) setField(key, parsed);
  }

  /** Parses the comma-separated array on every input, keeping the draft text. */
  function onStringArrayInput(key: string, raw: string): void {
    setDraft(key, raw);
    setField(
      key,
      raw
        .split(",")
        .map((part) => part.trim())
        .filter((part) => part !== ""),
    );
  }

  function toggleArrayValue(key: string, option: string, checked: boolean): void {
    const current = values[key];
    const list = Array.isArray(current) ? current : [];
    setField(key, checked ? [...list, option] : list.filter((value) => value !== option));
  }

  /** Content sent with `accept` (empty optional fields are omitted). */
  function buildContent(): Record<string, ElicitationValue> {
    const content: Record<string, ElicitationValue> = {};
    // Include hidden fields so their defaults reach agents that require them.
    for (const [key] of allProperties()) {
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

  async function respond(action: ElicitationAction): Promise<void> {
    if (busyAction() !== null) return;
    if (action === "accept" && missingRequired().length > 0) {
      toast.error(m.chat_required_fields({ fields: missingRequired().join(", ") }));
      return;
    }
    setBusyAction(action);
    try {
      const result = await sync.respondElicitation(
        props.sessionId,
        props.elicitationId,
        action,
        action === "accept" ? buildContent() : null,
      );
      if (result.already_resolved) {
        toast.info(m.error_already_resolved());
      } else {
        toast.success(m.chat_elicitation_sent());
      }
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusyAction(null);
    }
  }

  return (
    <div class="bg-card border-border flex flex-col gap-3 rounded-lg border p-3">
      <div class="flex min-w-0 flex-col gap-1">
        <div class="flex items-center gap-2">
          <IconCircleHelp class="text-signal size-4 shrink-0" />
          <span class="text-[13px] font-medium">{m.chat_elicitation_heading()}</span>
          <Show
            when={props.resolved}
            fallback={
              <Show when={props.createdAt !== null && props.createdAt !== undefined}>
                <span class="text-muted-foreground text-[13px]">
                  {formatRelativeTime(props.createdAt ?? null)}
                </span>
              </Show>
            }
          >
            {(resolved) => (
              <Badge variant="secondary">
                {resolvedLabel[resolved().action]()} ({resolved().resolvedBy})
              </Badge>
            )}
          </Show>
        </div>
        <p class="text-sm break-words">{props.message}</p>
      </div>

      <Show when={answerRows().length > 0}>
        <div class="bg-muted/50 border-border flex flex-col gap-2 rounded-md border p-2">
          <span class="text-muted-foreground text-[11px] font-medium">
            {m.chat_elicitation_your_answer()}
          </span>
          <dl class="flex flex-col gap-1.5">
            <For each={answerRows()}>
              {(row) => (
                <div class="flex flex-col gap-0.5">
                  <dt class="text-muted-foreground text-[13px]">{row.label}</dt>
                  <dd class="text-sm break-words whitespace-pre-wrap">{row.value}</dd>
                </div>
              )}
            </For>
          </dl>
        </div>
      </Show>

      <Show when={(props.resolved ?? null) === null}>
        <div class="flex flex-col gap-3">
          <For each={properties()}>
            {([key, property]) => {
              const inputId = `elicitation-${elicitationId()}-${key}`;
              return (
                <div class="flex flex-col gap-1.5">
                  <Label for={inputId}>
                    {property.title ?? key}
                    <Show when={required().has(key)}>
                      <span class="text-destructive">*</span>
                    </Show>
                  </Label>
                  <Show when={property.description}>
                    {(description) => (
                      <p class="text-muted-foreground text-[13px]">{description()}</p>
                    )}
                  </Show>
                  <Show
                    when={property.enum || property.oneOf}
                    fallback={
                      <Show
                        when={property.type === "boolean"}
                        fallback={
                          <Show
                            when={property.type === "array"}
                            fallback={
                              <Show
                                when={property.type === "integer" || property.type === "number"}
                                fallback={
                                  <Input
                                    id={inputId}
                                    value={
                                      typeof values[key] === "string" ? (values[key] as string) : ""
                                    }
                                    onValueChange={(value) => setField(key, value)}
                                  />
                                }
                              >
                                <Input
                                  id={inputId}
                                  type="number"
                                  value={drafts[key] ?? ""}
                                  onValueChange={(value) => onNumberInput(key, value)}
                                  onBlur={() => commitNumber(key)}
                                />
                              </Show>
                            }
                          >
                            <Show
                              when={property.items?.enum}
                              fallback={
                                <Input
                                  id={inputId}
                                  placeholder={m.chat_comma_separated()}
                                  value={drafts[key] ?? ""}
                                  onValueChange={(value) => onStringArrayInput(key, value)}
                                  onBlur={() => onStringArrayInput(key, drafts[key] ?? "")}
                                />
                              }
                            >
                              {(options) => (
                                <div class="flex flex-col gap-1.5">
                                  <For each={options()}>
                                    {(option) => (
                                      <label class="flex items-center gap-2 text-sm">
                                        <input
                                          type="checkbox"
                                          class="size-4"
                                          checked={
                                            Array.isArray(values[key]) &&
                                            (values[key] as string[]).includes(option)
                                          }
                                          onChange={(event) =>
                                            toggleArrayValue(
                                              key,
                                              option,
                                              event.currentTarget.checked,
                                            )
                                          }
                                        />
                                        <span>{option}</span>
                                      </label>
                                    )}
                                  </For>
                                </div>
                              )}
                            </Show>
                          </Show>
                        }
                      >
                        <label class="flex items-center gap-2 text-sm">
                          <input
                            id={inputId}
                            type="checkbox"
                            class="size-4"
                            checked={values[key] === true}
                            onChange={(event) => setField(key, event.currentTarget.checked)}
                          />
                          <span>{m.chat_yes()}</span>
                        </label>
                      </Show>
                    }
                  >
                    <select
                      id={inputId}
                      class="border-input bg-surface-raised h-9 rounded-md border px-2 text-sm"
                      value={typeof values[key] === "string" ? (values[key] as string) : ""}
                      onChange={(event) => setField(key, event.currentTarget.value)}
                    >
                      <option value="">{m.chat_select_option()}</option>
                      <For each={enumOptions(property)}>
                        {(option) => <option value={option.value}>{option.label}</option>}
                      </For>
                    </select>
                  </Show>
                </div>
              );
            }}
          </For>

          <div class="flex flex-wrap gap-2">
            <Button
              size="sm"
              disabled={busyAction() !== null || missingRequired().length > 0}
              onClick={() => void respond("accept")}
            >
              <IconSend />
              {m.chat_respond()}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busyAction() !== null}
              onClick={() => void respond("decline")}
            >
              <IconX />
              {m.chat_decline()}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={busyAction() !== null}
              onClick={() => void respond("cancel")}
            >
              {m.common_cancel()}
            </Button>
          </div>
        </div>
      </Show>
    </div>
  );
}
