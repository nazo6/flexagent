<script lang="ts">
  import { untrack } from "svelte";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import type {
    NodeSettingsSnapshot,
    ProjectProfile,
    SecretSummary,
    HubSettingsSnapshot,
  } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";

  type Section = "bootstrap" | "machine" | "server" | "projects";

  let { section, query = "" }: { section: Section; query?: string } = $props();

  const isServer = $derived(sync.connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));

  let selectedNode = $state("");
  let nodeSnapshot = $state<NodeSettingsSnapshot | null>(null);
  let hubSnapshot = $state<HubSettingsSnapshot | null>(null);
  let secrets = $state<SecretSummary[]>([]);
  let profiles = $state<Record<string, ProjectProfile>>({});
  let loading = $state(false);
  let error = $state<string | null>(null);

  // Node settings form
  let nodeForm = $state({
    name: "",
    allow_remote_pty: false,
    project_scan_dirs: "",
    tray: "" as "" | "true" | "false",
    revision: 0,
  });

  // Server settings form
  let serverForm = $state({ vapid_subject: "", revision: 0 });
  let gitCredentials = $state<
    { host: string; provider: string; username: string; detail: string }[]
  >([]);

  // Secret form
  let secretForm = $state({
    key: "",
    description: "",
    value: "",
  });

  // Project profile form
  let editingProfile = $state<string | null>(null);
  let profileForm = $state({ setup: "", env: "", revision: 0 });

  const targetNode = $derived(
    isServer ? selectedNode : (sync.connection.systemInfo?.node_id ?? ""),
  );
  const showSecrets = $derived(section === "machine" || (section === "server" && isServer));

  $effect(() => {
    if (isServer && selectedNode === "" && onlineNodes.length > 0) {
      selectedNode = onlineNodes[0].node_id;
    }
  });

  $effect(() => {
    // Track only the keys; `load` itself must not become an effect dependency
    // (otherwise setting `loading` would retrigger it).
    const key = `${section}:${targetNode}:${sync.connection.role}`;
    void key;
    untrack(() => void load());
  });

  async function load(): Promise<void> {
    if (loading) return;
    loading = true;
    error = null;
    try {
      if (section === "bootstrap" || section === "machine") {
        nodeSnapshot =
          targetNode !== "" ? await sync.connection.client.nodeSettings(targetNode) : null;
        syncNodeForm();
      }
      if (section === "bootstrap" || section === "machine" || section === "server") {
        if (isServer) {
          hubSnapshot = await sync.connection.client.hubSettings();
          syncServerForm();
        }
      }
      if (section === "machine") {
        secrets =
          targetNode !== "" ? (await sync.connection.client.nodeSecrets(targetNode)).secrets : [];
      } else if (section === "server" && isServer) {
        secrets = (await sync.connection.client.serverSecrets()).secrets;
      }
      if (section === "projects" && isServer) {
        await sync.refreshProjects();
        const entries = await Promise.all(
          sync.projects.map(async (project) => {
            const profile = await sync.connection.client.projectProfile(project.project_id);
            return [project.project_id, profile] as const;
          }),
        );
        profiles = Object.fromEntries(entries);
      }
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      loading = false;
    }
  }

  function syncNodeForm(): void {
    if (!nodeSnapshot) return;
    nodeForm = {
      name: nodeSnapshot.node.name,
      allow_remote_pty: nodeSnapshot.node.allow_remote_pty,
      project_scan_dirs: nodeSnapshot.node.project_scan_dirs.join("\n"),
      tray: nodeSnapshot.node.tray === null ? "" : nodeSnapshot.node.tray ? "true" : "false",
      revision: nodeSnapshot.node.revision,
    };
  }

  function syncServerForm(): void {
    if (!hubSnapshot) return;
    serverForm = {
      vapid_subject: hubSnapshot.hub.vapid_subject ?? "",
      revision: hubSnapshot.hub.revision,
    };
    gitCredentials = Object.entries(hubSnapshot.hub.git_credentials).map(([host, config]) => ({
      host,
      provider: config.provider,
      username: config.username ?? "",
      detail:
        config.provider === "env"
          ? (config as { token_env: string }).token_env
          : config.provider === "secret"
            ? (config as { secret_id: string }).secret_id
            : "",
    }));
  }

  async function saveNode(): Promise<void> {
    const request = {
      name: nodeForm.name.trim(),
      allow_remote_pty: nodeForm.allow_remote_pty,
      project_scan_dirs: nodeForm.project_scan_dirs
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line !== ""),
      tray: nodeForm.tray === "" ? null : nodeForm.tray === "true",
      expected_revision: nodeForm.revision,
    };
    try {
      if (targetNode === "") throw new Error("node is not selected");
      nodeSnapshot = await sync.connection.client.updateNodeSettings(targetNode, request);
      syncNodeForm();
      toast.success("Saved node settings.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function saveServer(): Promise<void> {
    try {
      hubSnapshot = await sync.connection.client.updateHubSettings({
        vapid_subject:
          serverForm.vapid_subject.trim() === "" ? null : serverForm.vapid_subject.trim(),
        git_credentials: Object.fromEntries(
          gitCredentials
            .filter((entry) => entry.host.trim() !== "")
            .map((entry) => [
              entry.host.trim(),
              entry.provider === "gh_cli"
                ? { provider: "gh_cli", username: entry.username || null }
                : entry.provider === "env"
                  ? {
                      provider: "env",
                      username: entry.username || null,
                      token_env: entry.detail,
                    }
                  : {
                      provider: "secret",
                      username: entry.username || null,
                      secret_id: entry.detail,
                    },
            ]),
        ),
        expected_revision: serverForm.revision,
      });
      syncServerForm();
      toast.success("Saved server settings.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function saveSecret(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const key = secretForm.key.trim();
    if (secretForm.value === "" || key === "") return;
    try {
      const request = {
        description: secretForm.description.trim() === "" ? null : secretForm.description.trim(),
        value: secretForm.value,
      };
      if (!(section === "server" && isServer) && targetNode === "") {
        throw new Error("node is not selected");
      }
      const response =
        section === "server" && isServer
          ? await sync.connection.client.upsertServerSecret(key, request)
          : await sync.connection.client.upsertNodeSecret(targetNode, key, request);
      secrets = response.secrets;
      secretForm = { key: "", description: "", value: "" };
      toast.success("Secret saved (the value is write-only and never returned).");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function deleteSecret(secret: SecretSummary): Promise<void> {
    if (!confirm(`Delete secret "${secret.id}"?`)) return;
    try {
      if (!(section === "server" && isServer) && targetNode === "") {
        throw new Error("node is not selected");
      }
      const response =
        section === "server" && isServer
          ? await sync.connection.client.deleteServerSecret(secret.id)
          : await sync.connection.client.deleteNodeSecret(targetNode, secret.id);
      secrets = response.secrets;
      toast.success(`Deleted secret "${secret.id}".`);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function startProfile(projectId: string): void {
    const profile = profiles[projectId];
    editingProfile = projectId;
    profileForm = {
      setup: profile.bootstrap_setup.join("\n"),
      env: Object.entries(profile.bootstrap_env)
        .map(([key, value]) => `${key}=${value}`)
        .join("\n"),
      revision: profile.revision,
    };
  }

  async function saveProfile(projectId: string): Promise<void> {
    const env: Record<string, string> = {};
    for (const line of profileForm.env.split("\n")) {
      const trimmed = line.trim();
      if (trimmed === "") continue;
      const index = trimmed.indexOf("=");
      if (index <= 0) continue;
      env[trimmed.slice(0, index)] = trimmed.slice(index + 1);
    }
    try {
      const profile = await sync.connection.client.updateProjectProfile(projectId, {
        bootstrap_setup: profileForm.setup
          .split("\n")
          .map((line) => line.trim())
          .filter((line) => line !== ""),
        bootstrap_env: env,
        expected_revision: profileForm.revision,
      });
      profiles = { ...profiles, [projectId]: profile };
      editingProfile = null;
      toast.success("Saved project profile.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  function sourceBadge(source: string): string {
    return source.replaceAll("_", " ");
  }

  type Row = { label: string; value: string; source: string; detail?: string | null };

  const bootstrapRows = $derived<Row[]>(
    section === "bootstrap"
      ? isServer
        ? hubSnapshot
          ? [
              {
                label: "Data directory",
                value: hubSnapshot.bootstrap.fxg_home.value,
                source: hubSnapshot.bootstrap.fxg_home.source,
              },
              {
                label: "Listen address",
                value: hubSnapshot.bootstrap.listen_addr.value,
                source: hubSnapshot.bootstrap.listen_addr.source,
              },
              {
                label: "Allowed hosts",
                value: hubSnapshot.bootstrap.allowed_hosts.value.join(", ") || "-",
                source: hubSnapshot.bootstrap.allowed_hosts.source,
              },
              {
                label: "Allowed origins",
                value: hubSnapshot.bootstrap.allowed_origins.value.join(", ") || "-",
                source: hubSnapshot.bootstrap.allowed_origins.source,
              },
              {
                label: "Log level",
                value: hubSnapshot.bootstrap.log_level.value,
                source: hubSnapshot.bootstrap.log_level.source,
              },
              {
                label: "Log file",
                value: hubSnapshot.bootstrap.log_file.value ?? "-",
                source: hubSnapshot.bootstrap.log_file.source,
              },
              {
                label: "Log format",
                value: hubSnapshot.bootstrap.log_format.value,
                source: hubSnapshot.bootstrap.log_format.source,
              },
              {
                label: "No color",
                value: String(hubSnapshot.bootstrap.log_no_color.value),
                source: hubSnapshot.bootstrap.log_no_color.source,
              },
            ]
          : []
        : nodeSnapshot
          ? [
              {
                label: "Data directory",
                value: nodeSnapshot.bootstrap.fxg_home.value,
                source: nodeSnapshot.bootstrap.fxg_home.source,
              },
              {
                label: "Node ID",
                value: nodeSnapshot.bootstrap.node_id.value,
                source: nodeSnapshot.bootstrap.node_id.source,
              },
              {
                label: "Listen address",
                value: nodeSnapshot.bootstrap.listen_addr.value,
                source: nodeSnapshot.bootstrap.listen_addr.source,
              },
              {
                label: "Hub",
                value: nodeSnapshot.bootstrap.hub_url.value ?? "-",
                source: nodeSnapshot.bootstrap.hub_url.source,
              },
              {
                label: "Log level",
                value: nodeSnapshot.bootstrap.log_level.value,
                source: nodeSnapshot.bootstrap.log_level.source,
              },
              {
                label: "Log file",
                value: nodeSnapshot.bootstrap.log_file.value ?? "-",
                source: nodeSnapshot.bootstrap.log_file.source,
              },
              {
                label: "Log format",
                value: nodeSnapshot.bootstrap.log_format.value,
                source: nodeSnapshot.bootstrap.log_format.source,
              },
              {
                label: "No color",
                value: String(nodeSnapshot.bootstrap.log_no_color.value),
                source: nodeSnapshot.bootstrap.log_no_color.source,
              },
            ]
          : []
      : [],
  );

  const filteredBootstrap = $derived(
    bootstrapRows.filter((row) =>
      `${row.label} ${row.value} ${row.source}`.toLowerCase().includes(query.toLowerCase()),
    ),
  );
</script>

<div class="flex flex-col gap-3">
  {#if isServer && (section === "bootstrap" || section === "machine")}
    <div class="flex flex-wrap items-center gap-2">
      <Label for="catalog-node">Target node</Label>
      <select
        id="catalog-node"
        class="border-input bg-background h-8 rounded-md border px-2 text-[13px]"
        bind:value={selectedNode}
      >
        {#each onlineNodes as node (node.node_id)}
          <option value={node.node_id}>{node.name} ({node.node_id})</option>
        {/each}
      </select>
      {#if selectedNode === ""}
        <span class="text-muted-foreground text-[13px]">
          No node is online. Node settings are read-only while offline.
        </span>
      {/if}
    </div>
  {/if}

  {#if error}
    <p class="text-destructive text-sm">{error}</p>
  {/if}

  {#if section === "bootstrap"}
    <Card class="gap-2 p-4">
      <h2 class="font-medium">Bootstrap (read-only)</h2>
      <p class="text-muted-foreground text-[13px]">
        Process start-up values from config.toml, environment variables and CLI flags. Change them
        in the file and restart the process.
      </p>
      {#each filteredBootstrap as row (row.label)}
        <div class="flex flex-wrap items-center gap-2 border-b py-1.5 last:border-b-0">
          <span class="w-40 text-[13px] font-medium">{row.label}</span>
          <code class="min-w-0 flex-1 truncate text-[13px]">{row.value}</code>
          <Badge variant="outline" class="text-[11px]">{sourceBadge(row.source)}</Badge>
        </div>
      {/each}
      {#if filteredBootstrap.length === 0}
        <p class="text-muted-foreground text-[13px]">
          {loading ? "Loading…" : "No matching values."}
        </p>
      {/if}
    </Card>
  {:else if section === "machine"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">This machine</h2>
      <p class="text-muted-foreground text-[13px]">
        Owned by <code>node.db</code>. The hub proxies writes to the target node; offline nodes are
        read-only.
      </p>
      {#if nodeSnapshot}
        <div class="grid gap-3 sm:grid-cols-2">
          <div class="grid gap-1.5">
            <Label for="node-name">Display name</Label>
            <Input id="node-name" bind:value={nodeForm.name} />
          </div>
          <div class="grid gap-1.5">
            <Label for="node-tray">Tray</Label>
            <select
              id="node-tray"
              class="border-input bg-background h-9 rounded-md border px-2 text-sm"
              bind:value={nodeForm.tray}
            >
              <option value="">Default</option>
              <option value="true">Enabled</option>
              <option value="false">Disabled</option>
            </select>
          </div>
          <label class="flex items-center gap-2 text-sm">
            <input type="checkbox" bind:checked={nodeForm.allow_remote_pty} />
            Allow remote Web PTY
          </label>
          <div class="grid gap-1.5 sm:col-span-2">
            <Label for="node-scan">Project scan directories (one per line)</Label>
            <textarea
              id="node-scan"
              class="border-input bg-background min-h-24 rounded-md border p-2 font-mono text-[13px]"
              bind:value={nodeForm.project_scan_dirs}></textarea>
          </div>
        </div>
        <div class="flex items-center gap-2">
          <Button
            size="sm"
            disabled={isServer && targetNode === ""}
            onclick={() => void saveNode()}
          >
            Save
          </Button>
          <span class="text-muted-foreground text-[13px]">revision {nodeForm.revision}</span>
        </div>
      {:else}
        <p class="text-muted-foreground text-[13px]">
          {loading ? "Loading…" : "Node settings are unavailable."}
        </p>
      {/if}
    </Card>
  {:else if section === "server"}
    {#if !isServer}
      <Card class="gap-2 p-4">
        <h2 class="font-medium">Server settings</h2>
        <p class="text-muted-foreground text-[13px]">
          Server-owned settings live in the hub's <code>hub.db</code>. Connect to the central server
          to edit them.
        </p>
      </Card>
    {:else}
      <Card class="gap-3 p-4">
        <h2 class="font-medium">Server</h2>
        <p class="text-muted-foreground text-[13px]">
          Owned by <code>hub.db</code>: Web Push contact and Git credential providers.
        </p>
        <div class="grid gap-1.5">
          <Label for="vapid-subject">VAPID subject</Label>
          <Input
            id="vapid-subject"
            placeholder="mailto:admin@example.com"
            bind:value={serverForm.vapid_subject}
          />
        </div>
        <div class="flex flex-col gap-2">
          <Label>Git credentials</Label>
          {#each gitCredentials as entry, index (index)}
            <div class="grid gap-2 sm:grid-cols-4">
              <Input placeholder="github.com" bind:value={entry.host} />
              <select
                class="border-input bg-background h-9 rounded-md border px-2 text-sm"
                bind:value={entry.provider}
              >
                <option value="gh_cli">gh CLI</option>
                <option value="env">Environment</option>
                <option value="secret">Secret</option>
              </select>
              <Input placeholder="username" bind:value={entry.username} />
              <Input
                placeholder={entry.provider === "env" ? "TOKEN_ENV" : "SECRET_KEY"}
                bind:value={entry.detail}
                disabled={entry.provider === "gh_cli"}
                list={entry.provider === "secret" ? "server-secrets-datalist" : undefined}
              />
            </div>
          {/each}
          <datalist id="server-secrets-datalist">
            {#each secrets as s (s.id)}
              <option value={s.id}>{s.description ? `${s.id} (${s.description})` : s.id}</option>
            {/each}
          </datalist>
          <div>
            <Button
              variant="outline"
              size="sm"
              onclick={() =>
                (gitCredentials = [
                  ...gitCredentials,
                  { host: "", provider: "gh_cli", username: "", detail: "" },
                ])}
            >
              Add Git credential
            </Button>
          </div>
        </div>
        <div class="flex items-center gap-2">
          <Button size="sm" onclick={() => void saveServer()}>Save</Button>
          <span class="text-muted-foreground text-[13px]">revision {serverForm.revision}</span>
        </div>
      </Card>
    {/if}
  {:else if section === "projects"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">Project profiles</h2>
      {#if !isServer}
        <p class="text-muted-foreground text-[13px]">
          Bootstrap profiles for temporary VMs are owned by the hub. Open the hub Web UI to edit
          them.
        </p>
      {:else}
        <p class="text-muted-foreground text-[13px]">
          Setup commands and environment variables injected into temporary VMs (<code
            >project_profiles</code
          >
          in <code>hub.db</code>).
        </p>
        {#each sync.projects as project (project.project_id)}
          {@const profile = profiles[project.project_id]}
          <div class="rounded-md border p-2">
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-medium">{project.name}</span>
              <code class="text-muted-foreground text-[11px]">{project.project_id}</code>
              <Badge variant="outline" class="text-[11px]">{project.identity.kind}</Badge>
              <Button
                variant="outline"
                size="sm"
                class="ml-auto"
                onclick={() => startProfile(project.project_id)}
              >
                Edit
              </Button>
            </div>
            {#if editingProfile === project.project_id}
              <div class="mt-2 grid gap-2">
                <div class="grid gap-1.5">
                  <Label for={`setup-${project.project_id}`}>Setup commands (one per line)</Label>
                  <textarea
                    id={`setup-${project.project_id}`}
                    class="border-input bg-background min-h-20 rounded-md border p-2 font-mono text-[13px]"
                    bind:value={profileForm.setup}></textarea>
                </div>
                <div class="grid gap-1.5">
                  <Label for={`env-${project.project_id}`}>Environment (KEY=VALUE)</Label>
                  <textarea
                    id={`env-${project.project_id}`}
                    class="border-input bg-background min-h-20 rounded-md border p-2 font-mono text-[13px]"
                    bind:value={profileForm.env}></textarea>
                </div>
                <div class="flex gap-2">
                  <Button size="sm" onclick={() => void saveProfile(project.project_id)}>
                    Save
                  </Button>
                  <Button variant="outline" size="sm" onclick={() => (editingProfile = null)}>
                    Cancel
                  </Button>
                </div>
              </div>
            {:else if profile}
              <p class="text-muted-foreground mt-1 text-[13px]">
                {profile.bootstrap_setup.length} setup command(s), {Object.keys(
                  profile.bootstrap_env,
                ).length} env var(s)
              </p>
            {/if}
          </div>
        {/each}
      {/if}
    </Card>
  {/if}

  {#if showSecrets}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">Secrets</h2>
      <p class="text-muted-foreground text-[13px]">
        Key-value secrets stored in <code>~/.flexagent/secrets/</code>. Values are write-only and
        never returned by the API. Reference them in custom agents as <code>@KEY</code> or in Git
        credentials as <code>KEY</code>.
      </p>
      {#if secrets.length > 0}
        <ul class="flex list-none flex-col gap-1 p-0 text-[13px]">
          {#each secrets as secret (secret.id)}
            <li class="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
              <span class="font-mono font-medium">{secret.id}</span>
              <Button
                variant="ghost"
                size="sm"
                class="h-6 px-1.5 text-[11px]"
                onclick={() => {
                  if (typeof navigator !== "undefined" && navigator.clipboard) {
                    void navigator.clipboard.writeText(secret.id);
                    toast.success(`Copied key "${secret.id}" to clipboard.`);
                  }
                }}
              >
                Copy key
              </Button>
              {#if secret.description}
                <span class="text-muted-foreground">{secret.description}</span>
              {/if}
              <Badge variant={secret.configured ? "secondary" : "outline"} class="text-[11px]">
                {secret.configured ? "Configured" : "Missing"}
              </Badge>
              {#if secret.used_by.length > 0}
                <span class="text-muted-foreground">used by {secret.used_by.join(", ")}</span>
              {/if}
              <div class="ml-auto flex gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  class="text-destructive"
                  onclick={() => void deleteSecret(secret)}
                  disabled={secret.used_by.length > 0}
                >
                  Delete
                </Button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
      <form class="grid gap-2 sm:grid-cols-4" onsubmit={saveSecret}>
        <Input placeholder="Key (e.g. OPENAI_API_KEY)" bind:value={secretForm.key} />
        <Input placeholder="Description (optional)" bind:value={secretForm.description} />
        <Input type="password" placeholder="Value (write-only)" bind:value={secretForm.value} />
        <Button type="submit" disabled={secretForm.key.trim() === "" || secretForm.value === ""}>
          Store secret
        </Button>
      </form>
    </Card>
  {/if}
</div>
