import {
  createEffect,
  createMemo,
  createSignal,
  createStore,
  For,
  Match,
  Show,
  Switch,
} from "solid-js";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import * as Select from "#lib/components/ui/select/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import type {
  HubSettingsSnapshot,
  NodeSettingsSnapshot,
  SecretSummary,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

type Section = "bootstrap" | "machine" | "hub";

interface NodeForm {
  name: string;
  allow_remote_pty: boolean;
  project_scan_dirs: string;
  tray: "" | "true" | "false";
  vapid_subject: string;
  revision: number;
}

interface HubForm {
  vapid_subject: string;
  revision: number;
}

interface SecretForm {
  key: string;
  description: string;
  value: string;
}

interface Row {
  label: string;
  value: string;
  source: string;
  detail?: string | null;
}

const emptyNodeForm = (): NodeForm => ({
  name: "",
  allow_remote_pty: false,
  project_scan_dirs: "",
  tray: "",
  vapid_subject: "",
  revision: 0,
});

const emptySecretForm = (): SecretForm => ({ key: "", description: "", value: "" });

/** Editable settings of the hub or a node, plus write-only secrets. */
export default function SettingsCatalog(props: { section: Section; nodeId?: string }) {
  const isHub = () => sync.connection.role === "hub";

  const [nodeSnapshot, setNodeSnapshot] = createSignal<NodeSettingsSnapshot | null>(null);
  const [hubSnapshot, setHubSnapshot] = createSignal<HubSettingsSnapshot | null>(null);
  const [secrets, setSecrets] = createSignal<SecretSummary[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  /** Guard against overlapping loads (plain flag: a signal write is only
   * visible after the flush, and the retry below runs in the same microtask). */
  let inFlight = false;
  /** Target whose snapshot / form is currently loaded (null = none). */
  const [loadedTarget, setLoadedTarget] = createSignal<string | null>(null);

  const [nodeForm, setNodeForm] = createStore<NodeForm>(emptyNodeForm());
  const [hubForm, setHubForm] = createStore<HubForm>({ vapid_subject: "", revision: 0 });
  const [secretForm, setSecretForm] = createStore<SecretForm>(emptySecretForm());

  const targetNode = createMemo(() =>
    isHub() ? (props.nodeId ?? "") : (sync.connection.systemInfo?.node_id ?? ""),
  );
  const showSecrets = createMemo(
    () => props.section === "machine" || (props.section === "hub" && isHub()),
  );

  createEffect(
    // Track only the keys; `load` itself must not become an effect dependency
    // (otherwise setting `loading` would retrigger it).
    () => `${props.section}:${targetNode()}:${sync.connection.role}`,
    () => {
      void load();
    },
  );

  async function load(): Promise<void> {
    if (inFlight) return;
    inFlight = true;
    // Capture the target: it can change while the requests are in flight.
    const node = targetNode();
    if (loadedTarget() !== node) {
      // Never keep the previous node's snapshot / form around: a failed load
      // must not let Save or Delete act on the new node with stale values.
      setNodeSnapshot(null);
      setHubSnapshot(null);
      setSecrets([]);
      setLoadedTarget(null);
    }
    setLoading(true);
    setError(null);
    try {
      if (props.section === "bootstrap" || props.section === "machine") {
        const snapshot = node !== "" ? await sync.connection.client.nodeSettings(node) : null;
        if (targetNode() !== node) return;
        setNodeSnapshot(snapshot);
        syncNodeForm(snapshot);
      }
      if (props.section === "bootstrap" || props.section === "machine" || props.section === "hub") {
        if (isHub()) {
          const snapshot = await sync.connection.client.hubSettings();
          if (targetNode() !== node) return;
          setHubSnapshot(snapshot);
          syncHubForm(snapshot);
        }
      }
      if (props.section === "machine") {
        const response =
          node !== "" ? await sync.connection.client.nodeSecrets(node) : { secrets: [] };
        if (targetNode() !== node) return;
        setSecrets(response.secrets);
      } else if (props.section === "hub" && isHub()) {
        const response = await sync.connection.client.hubSecrets();
        if (targetNode() !== node) return;
        setSecrets(response.secrets);
      }
      setLoadedTarget(node);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      inFlight = false;
      setLoading(false);
      // The target changed while loading; retry now that the guard is released.
      if (targetNode() !== node) void load();
    }
  }

  function syncNodeForm(current: NodeSettingsSnapshot | null = nodeSnapshot()): void {
    if (!current) return;
    const snapshot = current;
    setNodeForm((form) => {
      form.name = snapshot.node.name;
      form.allow_remote_pty = snapshot.node.allow_remote_pty;
      form.project_scan_dirs = snapshot.node.project_scan_dirs.join("\n");
      form.tray = snapshot.node.tray === null ? "" : snapshot.node.tray ? "true" : "false";
      form.vapid_subject = snapshot.node.vapid_subject ?? "";
      form.revision = snapshot.node.revision;
    });
  }

  function syncHubForm(current: HubSettingsSnapshot | null = hubSnapshot()): void {
    if (!current) return;
    const snapshot = current;
    setHubForm((form) => {
      form.vapid_subject = snapshot.hub.vapid_subject ?? "";
      form.revision = snapshot.hub.revision;
    });
  }

  async function saveNode(): Promise<void> {
    if (targetNode() === "" || loadedTarget() !== targetNode() || nodeSnapshot() === null) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    const request = {
      name: nodeForm.name.trim(),
      allow_remote_pty: nodeForm.allow_remote_pty,
      project_scan_dirs: nodeForm.project_scan_dirs
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line !== ""),
      tray: nodeForm.tray === "" ? null : nodeForm.tray === "true",
      vapid_subject: nodeForm.vapid_subject.trim() === "" ? null : nodeForm.vapid_subject.trim(),
      expected_revision: nodeForm.revision,
    };
    try {
      const updated = await sync.connection.client.updateNodeSettings(targetNode(), request);
      setNodeSnapshot(updated);
      syncNodeForm(updated);
      toast.success("Saved node settings.");
    } catch (cause) {
      toast.error(errorMessage(cause));
    }
  }

  async function saveHub(): Promise<void> {
    if (hubSnapshot() === null || loadedTarget() !== targetNode()) {
      toast.error(m.settings_hub_not_loaded());
      return;
    }
    try {
      const updated = await sync.connection.client.updateHubSettings({
        vapid_subject: hubForm.vapid_subject.trim() === "" ? null : hubForm.vapid_subject.trim(),
        expected_revision: hubForm.revision,
      });
      setHubSnapshot(updated);
      syncHubForm(updated);
      toast.success("Saved hub settings.");
    } catch (cause) {
      toast.error(errorMessage(cause));
    }
  }

  async function saveSecret(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const key = secretForm.key.trim();
    if (secretForm.value === "" || key === "") return;
    if (loadedTarget() !== targetNode()) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    try {
      const request = {
        description: secretForm.description.trim() === "" ? null : secretForm.description.trim(),
        value: secretForm.value,
      };
      if (!(props.section === "hub" && isHub()) && targetNode() === "") {
        throw new Error(m.settings_node_not_loaded());
      }
      const response =
        props.section === "hub" && isHub()
          ? await sync.connection.client.upsertHubSecret(key, request)
          : await sync.connection.client.upsertNodeSecret(targetNode(), key, request);
      setSecrets(response.secrets);
      setSecretForm((form) => {
        const next = emptySecretForm();
        form.key = next.key;
        form.description = next.description;
        form.value = next.value;
      });
      toast.success("Secret saved (the value is write-only and never returned).");
    } catch (cause) {
      toast.error(errorMessage(cause));
    }
  }

  async function deleteSecret(secret: SecretSummary): Promise<void> {
    if (loadedTarget() !== targetNode()) {
      toast.error(m.settings_node_not_loaded());
      return;
    }
    if (!confirm(`Delete secret "${secret.id}"?`)) return;
    try {
      if (!(props.section === "hub" && isHub()) && targetNode() === "") {
        throw new Error(m.settings_node_not_loaded());
      }
      const response =
        props.section === "hub" && isHub()
          ? await sync.connection.client.deleteHubSecret(secret.id)
          : await sync.connection.client.deleteNodeSecret(targetNode(), secret.id);
      setSecrets(response.secrets);
      toast.success(`Deleted secret "${secret.id}".`);
    } catch (cause) {
      toast.error(errorMessage(cause));
    }
  }

  function trayLabel(value: string): string {
    if (value === "true") return "Enabled";
    if (value === "false") return "Disabled";
    return "Default";
  }

  function sourceBadge(source: string): string {
    return source.replaceAll("_", " ");
  }

  // Bootstrap values describe a process: the hub's own when no node target is
  // set, otherwise the selected node's.
  const bootstrapRows = createMemo<Row[]>(() => {
    if (props.section !== "bootstrap") return [];
    if (isHub() && targetNode() === "") {
      const snapshot = hubSnapshot();
      if (!snapshot) return [];
      return [
        {
          label: "Data directory",
          value: snapshot.bootstrap.data_dir.value,
          source: snapshot.bootstrap.data_dir.source,
        },
        {
          label: "Listen address",
          value: snapshot.bootstrap.listen_addr.value,
          source: snapshot.bootstrap.listen_addr.source,
        },
        {
          label: "Allowed hosts",
          value: snapshot.bootstrap.allowed_hosts.value.join(", ") || "-",
          source: snapshot.bootstrap.allowed_hosts.source,
        },
        {
          label: "Allowed origins",
          value: snapshot.bootstrap.allowed_origins.value.join(", ") || "-",
          source: snapshot.bootstrap.allowed_origins.source,
        },
        {
          label: "Log level",
          value: snapshot.bootstrap.log_level.value,
          source: snapshot.bootstrap.log_level.source,
        },
        {
          label: "Log file",
          value: snapshot.bootstrap.log_file.value ?? "-",
          source: snapshot.bootstrap.log_file.source,
        },
        {
          label: "Log format",
          value: snapshot.bootstrap.log_format.value,
          source: snapshot.bootstrap.log_format.source,
        },
        {
          label: "No color",
          value: String(snapshot.bootstrap.log_no_color.value),
          source: snapshot.bootstrap.log_no_color.source,
        },
      ];
    }
    const snapshot = nodeSnapshot();
    if (!snapshot) return [];
    return [
      {
        label: "Data directory",
        value: snapshot.bootstrap.data_dir.value,
        source: snapshot.bootstrap.data_dir.source,
      },
      {
        label: "Node ID",
        value: snapshot.bootstrap.node_id.value,
        source: snapshot.bootstrap.node_id.source,
      },
      {
        label: "Listen address",
        value: snapshot.bootstrap.listen_addr.value,
        source: snapshot.bootstrap.listen_addr.source,
      },
      {
        label: "Allowed hosts",
        value: snapshot.bootstrap.allowed_hosts.value.join(", ") || "-",
        source: snapshot.bootstrap.allowed_hosts.source,
      },
      {
        label: "Allowed origins",
        value: snapshot.bootstrap.allowed_origins.value.join(", ") || "-",
        source: snapshot.bootstrap.allowed_origins.source,
      },
      {
        label: "Log level",
        value: snapshot.bootstrap.log_level.value,
        source: snapshot.bootstrap.log_level.source,
      },
      {
        label: "Log file",
        value: snapshot.bootstrap.log_file.value ?? "-",
        source: snapshot.bootstrap.log_file.source,
      },
      {
        label: "Log format",
        value: snapshot.bootstrap.log_format.value,
        source: snapshot.bootstrap.log_format.source,
      },
      {
        label: "No color",
        value: String(snapshot.bootstrap.log_no_color.value),
        source: snapshot.bootstrap.log_no_color.source,
      },
    ];
  });

  /** Data directory shown by the bootstrap tab (hub or target node). */
  const dataDir = createMemo(() =>
    isHub() && targetNode() === ""
      ? (hubSnapshot()?.bootstrap.data_dir.value ?? "")
      : (nodeSnapshot()?.bootstrap.data_dir.value ?? ""),
  );

  /** Files and directories in the role data directory, with what each one is for. */
  const dataFiles = createMemo<{ name: string; use: string }[]>(() =>
    isHub() && targetNode() === ""
      ? [
          { name: "auth_token", use: "Hub-only Web access token for browsers and CLI" },
          { name: "hub.db", use: "Hub settings, project projection and audit log" },
          { name: "vapid.json", use: "Web Push VAPID keypair" },
          { name: "secrets/", use: "Write-only secret values (0600)" },
        ]
      : [
          { name: "auth_token", use: "Node-only Web access token for browsers and CLI" },
          { name: "node_token", use: "Node pairing token (set by fxg node pair)" },
          { name: "node.db", use: "Node settings, sessions and audit log" },
          { name: "vapid.json", use: "Web Push VAPID keypair" },
          { name: "secrets/", use: "Write-only secret values (0600)" },
          { name: "worktrees/", use: "Git worktrees created by fxg" },
        ],
  );

  return (
    <div class="flex flex-col gap-3">
      <Show when={isHub() && props.section === "machine" && targetNode() === ""}>
        <p class="text-muted-foreground text-[13px]">{m.common_no_node_online()}</p>
      </Show>

      <Show when={error()}>
        <p class="text-destructive text-sm">{error()}</p>
      </Show>

      <Switch>
        <Match when={props.section === "bootstrap"}>
          <Card class="gap-2 p-4">
            <h2 class="font-medium">Bootstrap (read-only)</h2>
            <p class="text-muted-foreground text-[13px]">
              Process start-up values from node.toml / hub.toml, environment variables and CLI
              flags. Change them in the file and restart the process.
            </p>
            <For each={bootstrapRows()}>
              {(row) => (
                <div class="flex flex-wrap items-center gap-2 border-b py-1.5 last:border-b-0">
                  <span class="w-40 text-[13px] font-medium">{row.label}</span>
                  <code class="min-w-0 flex-1 truncate text-[13px]">{row.value}</code>
                  <Badge variant="outline" class="text-[11px]">
                    {sourceBadge(row.source)}
                  </Badge>
                </div>
              )}
            </For>
            <Show when={bootstrapRows().length === 0}>
              <p class="text-muted-foreground text-[13px]">
                {loading() ? "Loading…" : "No values available."}
              </p>
            </Show>
            <details class="border-t pt-2 text-[13px]">
              <summary class="hover:text-foreground cursor-pointer font-medium">
                Data directory contents
              </summary>
              <p class="text-muted-foreground mt-1 text-[12px]">
                Files under <code>{dataDir() || "the data directory"}</code>. Settings, runtime
                files and logs live in their own roots; <code>fxg doctor</code> lists every path.
                Runtime files are recreated on start; do not delete a file unless you know what it
                owns.
              </p>
              <ul class="mt-2 flex list-none flex-col gap-1 p-0">
                <For each={dataFiles()}>
                  {(file) => (
                    <li class="flex flex-wrap gap-x-2">
                      <code class="min-w-40 font-mono">{file.name}</code>
                      <span class="text-muted-foreground">{file.use}</span>
                    </li>
                  )}
                </For>
              </ul>
            </details>
          </Card>
        </Match>

        <Match when={props.section === "machine"}>
          <Card class="gap-3 p-4">
            <h2 class="font-medium">Node settings</h2>
            <p class="text-muted-foreground text-[13px]">
              Owned by <code>node.db</code> on the target node. Through the hub, writes are proxied
              and offline nodes are read-only.
            </p>
            <Show
              when={nodeSnapshot()}
              fallback={
                <p class="text-muted-foreground text-[13px]">
                  {loading() ? "Loading…" : "Node settings are unavailable."}
                </p>
              }
            >
              <div class="grid gap-3 sm:grid-cols-2">
                <div class="grid gap-1.5">
                  <Label for="node-name">Display name</Label>
                  <Input
                    id="node-name"
                    value={nodeForm.name}
                    onValueChange={(value) =>
                      setNodeForm((form) => {
                        form.name = value;
                      })
                    }
                  />
                </div>
                <div class="grid gap-1.5">
                  <Label for="node-tray">Tray</Label>
                  <Select.Root
                    type="single"
                    value={nodeForm.tray === "" ? "default" : nodeForm.tray}
                    onValueChange={(value) =>
                      setNodeForm((form) => {
                        form.tray = value === "default" ? "" : (String(value) as "true" | "false");
                      })
                    }
                  >
                    <Select.Trigger id="node-tray" size="sm" class="w-full text-[13px]">
                      <span>{trayLabel(nodeForm.tray)}</span>
                    </Select.Trigger>
                    <Select.Content>
                      <Select.Item value="default" label="Default" class="text-[13px]">
                        Default
                      </Select.Item>
                      <Select.Item value="true" label="Enabled" class="text-[13px]">
                        Enabled
                      </Select.Item>
                      <Select.Item value="false" label="Disabled" class="text-[13px]">
                        Disabled
                      </Select.Item>
                    </Select.Content>
                  </Select.Root>
                </div>
                <label class="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={nodeForm.allow_remote_pty}
                    onChange={(event) =>
                      setNodeForm((form) => {
                        form.allow_remote_pty = event.currentTarget.checked;
                      })
                    }
                  />
                  Allow remote Web PTY
                </label>
                <div class="grid gap-1.5 sm:col-span-2">
                  <Label for="node-vapid-subject">VAPID subject</Label>
                  <Input
                    id="node-vapid-subject"
                    placeholder="mailto:admin@example.com"
                    value={nodeForm.vapid_subject}
                    onValueChange={(value) =>
                      setNodeForm((form) => {
                        form.vapid_subject = value;
                      })
                    }
                  />
                </div>
                <div class="grid gap-1.5 sm:col-span-2">
                  <Label for="node-scan">Project scan directories (one per line)</Label>
                  <textarea
                    id="node-scan"
                    class="border-input bg-surface-raised min-h-24 rounded-md border p-2 font-mono text-[13px]"
                    value={nodeForm.project_scan_dirs}
                    onInput={(event) =>
                      setNodeForm((form) => {
                        form.project_scan_dirs = event.currentTarget.value;
                      })
                    }
                  ></textarea>
                </div>
              </div>
              <div class="flex items-center gap-2">
                <Button
                  size="sm"
                  disabled={(isHub() && targetNode() === "") || loadedTarget() !== targetNode()}
                  onClick={() => void saveNode()}
                >
                  Save
                </Button>
                <span class="text-muted-foreground text-[13px]">revision {nodeForm.revision}</span>
              </div>
            </Show>
          </Card>
        </Match>

        <Match when={props.section === "hub"}>
          <Show
            when={isHub()}
            fallback={
              <Card class="gap-2 p-4">
                <h2 class="font-medium">Hub settings</h2>
                <p class="text-muted-foreground text-[13px]">
                  Hub-owned settings live in <code>hub.db</code>. Connect to the hub to edit them.
                </p>
              </Card>
            }
          >
            <Card class="gap-3 p-4">
              <h2 class="font-medium">Hub</h2>
              <p class="text-muted-foreground text-[13px]">
                Owned by <code>hub.db</code>: the Web Push contact claim.
              </p>
              <div class="grid gap-1.5">
                <Label for="vapid-subject">VAPID subject</Label>
                <Input
                  id="vapid-subject"
                  placeholder="mailto:admin@example.com"
                  value={hubForm.vapid_subject}
                  onValueChange={(value) =>
                    setHubForm((form) => {
                      form.vapid_subject = value;
                    })
                  }
                />
              </div>
              <div class="flex items-center gap-2">
                <Button size="sm" disabled={hubSnapshot() === null} onClick={() => void saveHub()}>
                  Save
                </Button>
                <span class="text-muted-foreground text-[13px]">revision {hubForm.revision}</span>
              </div>
            </Card>
          </Show>
        </Match>
      </Switch>

      <Show when={showSecrets()}>
        <Card class="gap-3 p-4">
          <h2 class="font-medium">Secrets</h2>
          <p class="text-muted-foreground text-[13px]">
            Key-value secrets stored in the role data directory under <code>secrets/</code>. Values
            are write-only and never returned by the API. Reference them in custom agents as{" "}
            <code>@KEY</code> or in provisioner environment variables as <code>KEY</code>.
          </p>
          <Show when={secrets().length > 0}>
            <ul class="flex list-none flex-col gap-1 p-0 text-[13px]">
              <For each={secrets()}>
                {(secret) => (
                  <li class="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
                    <span class="font-mono font-medium">{secret.id}</span>
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-6 px-1.5 text-[11px]"
                      onClick={() => {
                        if (typeof navigator !== "undefined" && navigator.clipboard) {
                          void navigator.clipboard.writeText(secret.id);
                          toast.success(`Copied key "${secret.id}" to clipboard.`);
                        }
                      }}
                    >
                      Copy key
                    </Button>
                    <Show when={secret.description}>
                      <span class="text-muted-foreground">{secret.description}</span>
                    </Show>
                    <Badge
                      variant={secret.configured ? "secondary" : "outline"}
                      class="text-[11px]"
                    >
                      {secret.configured ? "Configured" : "Missing"}
                    </Badge>
                    <Show when={secret.used_by.length > 0}>
                      <span class="text-muted-foreground">used by {secret.used_by.join(", ")}</span>
                    </Show>
                    <div class="ml-auto flex gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        class="text-destructive"
                        onClick={() => void deleteSecret(secret)}
                        disabled={secret.used_by.length > 0}
                      >
                        Delete
                      </Button>
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <form class="grid gap-2 sm:grid-cols-4" onSubmit={(event) => void saveSecret(event)}>
            <Input
              placeholder="Key (e.g. OPENAI_API_KEY)"
              aria-label={m.settings_secret_key_label()}
              value={secretForm.key}
              onValueChange={(value) =>
                setSecretForm((form) => {
                  form.key = value;
                })
              }
            />
            <Input
              placeholder="Description (optional)"
              aria-label={m.settings_secret_description_label()}
              value={secretForm.description}
              onValueChange={(value) =>
                setSecretForm((form) => {
                  form.description = value;
                })
              }
            />
            <Input
              type="password"
              placeholder="Value (write-only)"
              aria-label={m.settings_secret_value_label()}
              autocomplete="new-password"
              value={secretForm.value}
              onValueChange={(value) =>
                setSecretForm((form) => {
                  form.value = value;
                })
              }
            />
            <Button
              type="submit"
              disabled={secretForm.key.trim() === "" || secretForm.value === ""}
            >
              Store secret
            </Button>
          </form>
        </Card>
      </Show>
    </div>
  );
}
