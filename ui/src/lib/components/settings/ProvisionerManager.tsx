import { createMemo, createSignal, createStore, For, onSettled, Show } from "solid-js";
import IconPencil from "~icons/lucide/pencil";
import IconPlay from "~icons/lucide/play";
import IconPlus from "~icons/lucide/plus";
import IconTrash2 from "~icons/lucide/trash-2";
import { errorMessage } from "#lib/api/error-messages.ts";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type { ProvisionerDefinition, ProvisionerSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

interface ProvisionerForm {
  name: string;
  description: string;
  command: string;
  args: string;
  idleTimeout: string;
  env: string;
  envSecrets: string;
}

interface TestResult {
  name: string;
  ok: boolean;
  logLines: string[];
  error: string | null;
}

const emptyForm = (): ProvisionerForm => ({
  name: "",
  description: "",
  command: "",
  args: "",
  idleTimeout: "",
  env: "",
  envSecrets: "",
});

/** Hub-side provisioners (Docker / Incus / ephemeral VMs). */
export default function ProvisionerManager() {
  const [provisioners, setProvisioners] = createSignal<ProvisionerSummary[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [testingName, setTestingName] = createSignal<string | null>(null);
  const [editorOpen, setEditorOpen] = createSignal(false);
  const [editingName, setEditingName] = createSignal<string | null>(null);
  const [removeTarget, setRemoveTarget] = createSignal<ProvisionerSummary | null>(null);
  const [testResult, setTestResult] = createSignal<TestResult | null>(null);

  const [form, setForm] = createStore<ProvisionerForm>(emptyForm());

  const busy = createMemo(() => saving() || testingName() !== null);

  onSettled(() => {
    void loadProvisioners();
  });

  async function loadProvisioners(): Promise<void> {
    if (loading()) return;
    setLoading(true);
    try {
      setProvisioners(await sync.connection.client.provisioners());
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setLoading(false);
    }
  }

  function resetForm(): void {
    setEditingName(null);
    const next = emptyForm();
    setForm((current) => {
      current.name = next.name;
      current.description = next.description;
      current.command = next.command;
      current.args = next.args;
      current.idleTimeout = next.idleTimeout;
      current.env = next.env;
      current.envSecrets = next.envSecrets;
    });
  }

  function openCreate(): void {
    resetForm();
    setEditorOpen(true);
  }

  function openEdit(provisioner: ProvisionerSummary): void {
    setEditingName(provisioner.name);
    setForm((current) => {
      current.name = provisioner.name;
      current.description = provisioner.description ?? "";
      current.command = provisioner.command;
      current.args = provisioner.args.join("\n");
      current.idleTimeout = provisioner.idle_timeout_secs?.toString() ?? "";
      current.env = mapToLines(provisioner.env);
      current.envSecrets = mapToLines(provisioner.env_secrets);
    });
    setEditorOpen(true);
  }

  function mapToLines(map: Record<string, string>): string {
    return Object.entries(map)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n");
  }

  function parseLines(value: string): string[] {
    return value
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "");
  }

  function parseMap(value: string): Record<string, string> {
    const map: Record<string, string> = {};
    for (const line of parseLines(value)) {
      const separator = line.indexOf("=");
      if (separator <= 0) {
        throw new Error(`Expected KEY=VALUE: ${line}`);
      }
      const key = line.slice(0, separator).trim();
      const entry = line.slice(separator + 1).trim();
      if (key === "") {
        throw new Error(`Expected KEY=VALUE: ${line}`);
      }
      map[key] = entry;
    }
    return map;
  }

  async function save(): Promise<void> {
    if (saving()) return;
    const name = form.name.trim();
    if (name === "" || form.command.trim() === "") {
      toast.error(m.settings_provisioner_required());
      return;
    }
    setSaving(true);
    try {
      const idle = form.idleTimeout.trim();
      const definition: ProvisionerDefinition = {
        name,
        description: form.description.trim() === "" ? null : form.description.trim(),
        command: form.command.trim(),
        args: parseLines(form.args),
        idle_timeout_secs: idle === "" ? null : Number(idle),
        env: parseMap(form.env),
        env_secrets: parseMap(form.envSecrets),
      };
      setProvisioners(await sync.connection.client.upsertProvisioner(name, definition));
      toast.success(m.settings_provisioner_created({ name }));
      setEditorOpen(false);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSaving(false);
    }
  }

  async function runTest(provisioner: ProvisionerSummary): Promise<void> {
    if (busy()) return;
    setTestingName(provisioner.name);
    try {
      const result = await sync.connection.client.testProvisioner(provisioner.name);
      setTestResult({
        name: provisioner.name,
        ok: result.ok,
        logLines: result.log_lines,
        error: result.error,
      });
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setTestingName(null);
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget();
    if (target === null || saving()) return;
    setSaving(true);
    try {
      await sync.connection.client.deleteProvisioner(target.name);
      setProvisioners((current) => current.filter((entry) => entry.name !== target.name));
      toast.success(m.settings_provisioner_deleted({ name: target.name }));
      setRemoveTarget(null);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <Card class="gap-4 p-4">
        <div class="flex flex-wrap items-start justify-between gap-3">
          <div class="flex flex-col gap-1">
            <h2 class="font-medium">{m.settings_tab_provisioners()}</h2>
            <p class="text-muted-foreground text-[13px]">{m.settings_provisioners_description()}</p>
          </div>
          <Button size="sm" disabled={busy()} onClick={openCreate}>
            <IconPlus />
            {m.settings_provisioner_add()}
          </Button>
        </div>

        <Show
          when={!(loading() && provisioners().length === 0)}
          fallback={<p class="text-muted-foreground text-[13px]">{m.common_loading()}</p>}
        >
          <Show
            when={provisioners().length !== 0}
            fallback={
              <p class="text-muted-foreground text-[13px]">{m.settings_provisioners_empty()}</p>
            }
          >
            <ul class="flex list-none flex-col gap-2 p-0">
              <For each={provisioners()}>
                {(provisioner) => (
                  <li class="flex flex-wrap items-start gap-3 rounded-md border px-3 py-2.5">
                    <div class="flex min-w-0 flex-1 flex-col gap-1">
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="font-medium">{provisioner.name}</span>
                        <Badge variant="outline" class="font-mono text-[11px]">
                          {provisioner.command}
                        </Badge>
                        <Show when={provisioner.idle_timeout_secs !== null}>
                          <span class="text-muted-foreground text-[11px]">
                            {provisioner.idle_timeout_secs}s
                          </span>
                        </Show>
                      </div>
                      <Show when={provisioner.description}>
                        <p class="text-muted-foreground text-[12px]">{provisioner.description}</p>
                      </Show>
                      <Show when={provisioner.args.length > 0}>
                        <p class="text-muted-foreground truncate font-mono text-[11px]">
                          {provisioner.args.join(" ")}
                        </p>
                      </Show>
                    </div>
                    <div class="flex shrink-0 items-center gap-1">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={busy()}
                        onClick={() => void runTest(provisioner)}
                      >
                        <IconPlay />
                        {testingName() === provisioner.name
                          ? m.settings_provisioner_testing()
                          : m.settings_provisioner_test()}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        disabled={busy()}
                        onClick={() => openEdit(provisioner)}
                        title={m.settings_provisioner_edit({ name: provisioner.name })}
                      >
                        <IconPencil />
                        <span class="sr-only">
                          {m.settings_provisioner_edit({ name: provisioner.name })}
                        </span>
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        class="text-muted-foreground hover:text-destructive"
                        disabled={busy()}
                        onClick={() => setRemoveTarget(provisioner)}
                        title={m.common_delete()}
                      >
                        <IconTrash2 />
                        <span class="sr-only">{m.common_delete()}</span>
                      </Button>
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Show>
      </Card>

      <Dialog.Root open={editorOpen()} onOpenChange={setEditorOpen}>
        <Dialog.Content class="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden sm:max-w-lg">
          <Dialog.Header>
            <Dialog.Title>
              {editingName() === null
                ? m.settings_provisioner_add_title()
                : m.settings_provisioner_edit_title({ name: editingName() ?? "" })}
            </Dialog.Title>
            <Dialog.Description>{m.settings_provisioners_description()}</Dialog.Description>
          </Dialog.Header>

          <form
            class="flex min-h-0 flex-1 flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-1">
              <div class="grid gap-1.5">
                <Label for="provisioner-name">{m.settings_provisioner_name()}</Label>
                <Input
                  id="provisioner-name"
                  value={form.name}
                  onValueChange={(value) =>
                    setForm((draft) => {
                      draft.name = value;
                    })
                  }
                  disabled={editingName() !== null}
                  placeholder="local-docker"
                  class="font-mono text-[13px]"
                />
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-description">{m.settings_description()}</Label>
                <Input
                  id="provisioner-description"
                  value={form.description}
                  onValueChange={(value) =>
                    setForm((draft) => {
                      draft.description = value;
                    })
                  }
                  placeholder="Local Docker sandbox"
                  class="text-[13px]"
                />
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-command">{m.settings_command()}</Label>
                <Input
                  id="provisioner-command"
                  value={form.command}
                  onValueChange={(value) =>
                    setForm((draft) => {
                      draft.command = value;
                    })
                  }
                  placeholder="docker"
                  class="font-mono text-[13px]"
                />
                <p class="text-muted-foreground text-[11px]">{m.settings_command_hint()}</p>
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-args">{m.settings_args()}</Label>
                <textarea
                  id="provisioner-args"
                  rows={4}
                  class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
                  value={form.args}
                  onInput={(event) =>
                    setForm((draft) => {
                      draft.args = event.currentTarget.value;
                    })
                  }
                  placeholder={"run\n--rm\n-i\nfxg-sandbox:latest"}
                ></textarea>
                <p class="text-muted-foreground text-[11px]">
                  {m.settings_provisioner_args_hint()}
                </p>
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-idle">{m.settings_provisioner_idle_timeout()}</Label>
                <Input
                  id="provisioner-idle"
                  value={form.idleTimeout}
                  onValueChange={(value) =>
                    setForm((draft) => {
                      draft.idleTimeout = value;
                    })
                  }
                  type="number"
                  min="1"
                  placeholder="900"
                  class="font-mono text-[13px]"
                />
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-env">{m.settings_env_vars()}</Label>
                <textarea
                  id="provisioner-env"
                  rows={3}
                  class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
                  value={form.env}
                  onInput={(event) =>
                    setForm((draft) => {
                      draft.env = event.currentTarget.value;
                    })
                  }
                  placeholder="LOG_LEVEL=info"
                ></textarea>
                <p class="text-muted-foreground text-[11px]">
                  {m.settings_env_vars_hint()} <code>KEY=VALUE</code>
                </p>
              </div>
              <div class="grid gap-1.5">
                <Label for="provisioner-env-secrets">{m.settings_provisioner_env_secrets()}</Label>
                <textarea
                  id="provisioner-env-secrets"
                  rows={3}
                  class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
                  value={form.envSecrets}
                  onInput={(event) =>
                    setForm((draft) => {
                      draft.envSecrets = event.currentTarget.value;
                    })
                  }
                  placeholder="GH_TOKEN=GH_TOKEN"
                ></textarea>
                <p class="text-muted-foreground text-[11px]">
                  {m.settings_provisioner_env_secrets_hint()}
                </p>
              </div>
            </div>
            <Dialog.Footer class="shrink-0">
              <Dialog.Close>{m.common_cancel()}</Dialog.Close>
              <Button type="submit" disabled={saving()}>
                {saving() ? m.settings_saving() : m.common_save()}
              </Button>
            </Dialog.Footer>
          </form>
        </Dialog.Content>
      </Dialog.Root>

      <Dialog.Root
        open={testResult() !== null}
        onOpenChange={(open) => {
          if (!open) setTestResult(null);
        }}
      >
        <Dialog.Content class="sm:max-w-lg">
          <Dialog.Header>
            <Dialog.Title>
              {m.settings_provisioner_test_title({ name: testResult()?.name ?? "" })}
            </Dialog.Title>
            <Dialog.Description>
              {testResult()?.ok
                ? m.settings_provisioner_test_ok()
                : (testResult()?.error ?? m.settings_provisioner_test_failed())}
            </Dialog.Description>
          </Dialog.Header>
          <Show when={testResult()}>
            {(result) => (
              <Show when={result().logLines.length > 0}>
                <div class="flex flex-col gap-1">
                  <h3 class="text-sm font-medium">{m.settings_provisioner_logs()}</h3>
                  <pre class="bg-muted max-h-72 overflow-auto rounded-md p-2 font-mono text-[11px] whitespace-pre-wrap">
                    {result().logLines.join("\n")}
                  </pre>
                </div>
              </Show>
            )}
          </Show>
          <Dialog.Footer>
            <Dialog.Close>{m.common_close()}</Dialog.Close>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Root>

      <AlertDialog.Root
        open={removeTarget() !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
      >
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title>{m.settings_provisioner_remove_title()}</AlertDialog.Title>
            <AlertDialog.Description>
              {m.settings_provisioner_remove_description({ name: removeTarget()?.name ?? "" })}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel disabled={saving()}>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action
              variant="destructive"
              disabled={saving()}
              onClick={() => void confirmRemove()}
            >
              {m.common_delete()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
