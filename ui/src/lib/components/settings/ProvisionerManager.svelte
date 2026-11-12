<script lang="ts">
  import { onMount } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type { ProvisionerDefinition, ProvisionerSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import PencilIcon from "@lucide/svelte/icons/pencil";
  import PlayIcon from "@lucide/svelte/icons/play";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  let provisioners = $state<ProvisionerSummary[]>([]);
  let loading = $state(false);
  let saving = $state(false);
  let testingName = $state<string | null>(null);
  let editorOpen = $state(false);
  let editingName = $state<string | null>(null);
  let removeTarget = $state<ProvisionerSummary | null>(null);
  let testResult = $state<{
    name: string;
    ok: boolean;
    logLines: string[];
    error: string | null;
  } | null>(null);

  let form = $state({
    name: "",
    description: "",
    command: "",
    args: "",
    idleTimeout: "",
    env: "",
    envSecrets: "",
  });

  const busy = $derived(saving || testingName !== null);

  onMount(() => {
    void loadProvisioners();
  });

  async function loadProvisioners(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      provisioners = await sync.connection.client.provisioners();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      loading = false;
    }
  }

  function resetForm(): void {
    editingName = null;
    form = {
      name: "",
      description: "",
      command: "",
      args: "",
      idleTimeout: "",
      env: "",
      envSecrets: "",
    };
  }

  function openCreate(): void {
    resetForm();
    editorOpen = true;
  }

  function openEdit(provisioner: ProvisionerSummary): void {
    editingName = provisioner.name;
    form = {
      name: provisioner.name,
      description: provisioner.description ?? "",
      command: provisioner.command,
      args: provisioner.args.join("\n"),
      idleTimeout: provisioner.idle_timeout_secs?.toString() ?? "",
      env: mapToLines(provisioner.env),
      envSecrets: mapToLines(provisioner.env_secrets),
    };
    editorOpen = true;
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
    if (saving) return;
    const name = form.name.trim();
    if (name === "" || form.command.trim() === "") {
      toast.error(m.settings_provisioner_required());
      return;
    }
    saving = true;
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
      provisioners = await sync.connection.client.upsertProvisioner(name, definition);
      toast.success(m.settings_provisioner_created({ name }));
      editorOpen = false;
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      saving = false;
    }
  }

  async function runTest(provisioner: ProvisionerSummary): Promise<void> {
    if (busy) return;
    testingName = provisioner.name;
    try {
      const result = await sync.connection.client.testProvisioner(provisioner.name);
      testResult = {
        name: provisioner.name,
        ok: result.ok,
        logLines: result.log_lines,
        error: result.error,
      };
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      testingName = null;
    }
  }

  async function confirmRemove(): Promise<void> {
    const target = removeTarget;
    if (target === null || saving) return;
    saving = true;
    try {
      await sync.connection.client.deleteProvisioner(target.name);
      provisioners = provisioners.filter((entry) => entry.name !== target.name);
      toast.success(m.settings_provisioner_deleted({ name: target.name }));
      removeTarget = null;
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      saving = false;
    }
  }
</script>

<Card class="gap-4 p-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div class="flex flex-col gap-1">
      <h2 class="font-medium">{m.settings_tab_provisioners()}</h2>
      <p class="text-muted-foreground text-[13px]">
        {m.settings_provisioners_description()}
      </p>
    </div>
    <Button size="sm" disabled={busy} onclick={openCreate}>
      <PlusIcon />
      {m.settings_provisioner_add()}
    </Button>
  </div>

  {#if loading && provisioners.length === 0}
    <p class="text-muted-foreground text-[13px]">{m.common_loading()}</p>
  {:else if provisioners.length === 0}
    <p class="text-muted-foreground text-[13px]">{m.settings_provisioners_empty()}</p>
  {:else}
    <ul class="flex list-none flex-col gap-2 p-0">
      {#each provisioners as provisioner (provisioner.name)}
        <li class="flex flex-wrap items-start gap-3 rounded-md border px-3 py-2.5">
          <div class="flex min-w-0 flex-1 flex-col gap-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium">{provisioner.name}</span>
              <Badge variant="outline" class="font-mono text-[11px]">{provisioner.command}</Badge>
              {#if provisioner.idle_timeout_secs !== null}
                <span class="text-muted-foreground text-[11px]">
                  {provisioner.idle_timeout_secs}s
                </span>
              {/if}
            </div>
            {#if provisioner.description}
              <p class="text-muted-foreground text-[12px]">{provisioner.description}</p>
            {/if}
            {#if provisioner.args.length > 0}
              <p class="text-muted-foreground truncate font-mono text-[11px]">
                {provisioner.args.join(" ")}
              </p>
            {/if}
          </div>
          <div class="flex shrink-0 items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onclick={() => void runTest(provisioner)}
            >
              <PlayIcon />
              {testingName === provisioner.name
                ? m.settings_provisioner_testing()
                : m.settings_provisioner_test()}
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              disabled={busy}
              onclick={() => openEdit(provisioner)}
              title={m.settings_provisioner_edit({ name: provisioner.name })}
            >
              <PencilIcon />
              <span class="sr-only">{m.settings_provisioner_edit({ name: provisioner.name })}</span>
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              class="text-muted-foreground hover:text-destructive"
              disabled={busy}
              onclick={() => (removeTarget = provisioner)}
              title={m.common_delete()}
            >
              <Trash2Icon />
              <span class="sr-only">{m.common_delete()}</span>
            </Button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</Card>

<Dialog.Root open={editorOpen} onOpenChange={(open) => (editorOpen = open)}>
  <Dialog.Content class="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {editingName === null
          ? m.settings_provisioner_add_title()
          : m.settings_provisioner_edit_title({ name: editingName })}
      </Dialog.Title>
      <Dialog.Description>{m.settings_provisioners_description()}</Dialog.Description>
    </Dialog.Header>

    <form
      class="flex min-h-0 flex-1 flex-col gap-3"
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-1">
        <div class="grid gap-1.5">
          <Label for="provisioner-name">{m.settings_provisioner_name()}</Label>
          <Input
            id="provisioner-name"
            bind:value={form.name}
            disabled={editingName !== null}
            placeholder="local-docker"
            class="font-mono text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="provisioner-description">{m.settings_description()}</Label>
          <Input
            id="provisioner-description"
            bind:value={form.description}
            placeholder="Local Docker sandbox"
            class="text-[13px]"
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="provisioner-command">{m.settings_command()}</Label>
          <Input
            id="provisioner-command"
            bind:value={form.command}
            placeholder="docker"
            class="font-mono text-[13px]"
          />
          <p class="text-muted-foreground text-[11px]">{m.settings_command_hint()}</p>
        </div>
        <div class="grid gap-1.5">
          <Label for="provisioner-args">{m.settings_args()}</Label>
          <textarea
            id="provisioner-args"
            bind:value={form.args}
            rows="4"
            class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
            placeholder="run&#10;--rm&#10;-i&#10;fxg-sandbox:latest"></textarea>
          <p class="text-muted-foreground text-[11px]">{m.settings_provisioner_args_hint()}</p>
        </div>
        <div class="grid gap-1.5">
          <Label for="provisioner-idle">{m.settings_provisioner_idle_timeout()}</Label>
          <Input
            id="provisioner-idle"
            bind:value={form.idleTimeout}
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
            bind:value={form.env}
            rows="3"
            class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
            placeholder="LOG_LEVEL=info"></textarea>
          <p class="text-muted-foreground text-[11px]">
            {m.settings_env_vars_hint()} <code>KEY=VALUE</code>
          </p>
        </div>
        <div class="grid gap-1.5">
          <Label for="provisioner-env-secrets">{m.settings_provisioner_env_secrets()}</Label>
          <textarea
            id="provisioner-env-secrets"
            bind:value={form.envSecrets}
            rows="3"
            class="border-input bg-surface-raised focus-visible:ring-ring/40 rounded-md border px-2 py-1.5 font-mono text-[13px] outline-none focus-visible:ring-1"
            placeholder="GH_TOKEN=GH_TOKEN"></textarea>
          <p class="text-muted-foreground text-[11px]">
            {m.settings_provisioner_env_secrets_hint()}
          </p>
        </div>
      </div>
      <Dialog.Footer class="shrink-0">
        <Dialog.Close>{m.common_cancel()}</Dialog.Close>
        <Button type="submit" disabled={saving}>
          {saving ? m.settings_saving() : m.common_save()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<Dialog.Root open={testResult !== null} onOpenChange={(open) => !open && (testResult = null)}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {m.settings_provisioner_test_title({ name: testResult?.name ?? "" })}
      </Dialog.Title>
      <Dialog.Description>
        {testResult?.ok
          ? m.settings_provisioner_test_ok()
          : (testResult?.error ?? m.settings_provisioner_test_failed())}
      </Dialog.Description>
    </Dialog.Header>
    {#if testResult && testResult.logLines.length > 0}
      <div class="flex flex-col gap-1">
        <h3 class="text-sm font-medium">{m.settings_provisioner_logs()}</h3>
        <pre
          class="bg-muted max-h-72 overflow-auto rounded-md p-2 font-mono text-[11px] whitespace-pre-wrap">{testResult.logLines.join(
            "\n",
          )}</pre>
      </div>
    {/if}
    <Dialog.Footer>
      <Dialog.Close>{m.common_close()}</Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root
  open={removeTarget !== null}
  onOpenChange={(open) => !open && (removeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.settings_provisioner_remove_title()}</AlertDialog.Title>
      <AlertDialog.Description>
        {m.settings_provisioner_remove_description({ name: removeTarget?.name ?? "" })}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={saving}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={saving}
        onclick={() => void confirmRemove()}
      >
        {m.common_delete()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
