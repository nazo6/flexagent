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
    ServerSettingsSnapshot,
  } from "#lib/generated/bindings.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";

  type Section = "bootstrap" | "machine" | "server" | "projects";

  let { section, query = "" }: { section: Section; query?: string } = $props();

  const isServer = $derived(sync.connection.role === "central_server");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));

  let selectedNode = $state("");
  let nodeSnapshot = $state<NodeSettingsSnapshot | null>(null);
  let serverSnapshot = $state<ServerSettingsSnapshot | null>(null);
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
    name: "",
    kind: "agent_env",
    value: "",
  });

  // Project profile form
  let editingProfile = $state<string | null>(null);
  let profileForm = $state({ setup: "", env: "", revision: 0 });

  const targetNode = $derived(isServer ? selectedNode : "");

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
        if (isServer) {
          if (targetNode !== "") {
            nodeSnapshot = await sync.connection.client.nodeSettingsOf(targetNode);
          } else {
            nodeSnapshot = null;
          }
        } else {
          nodeSnapshot = await sync.connection.client.nodeSettings();
        }
        syncNodeForm();
      }
      if (section === "machine" || section === "server") {
        if (isServer) {
          serverSnapshot = await sync.connection.client.serverSettings();
          syncServerForm();
        }
      }
      if (section === "bootstrap" || section === "machine" || section === "server") {
        const response =
          isServer && section === "machine" && targetNode !== ""
            ? await sync.connection.client.nodeSecrets(targetNode)
            : await sync.connection.client.secrets();
        secrets = response.secrets;
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
    if (!serverSnapshot) return;
    serverForm = {
      vapid_subject: serverSnapshot.server.vapid_subject ?? "",
      revision: serverSnapshot.server.revision,
    };
    gitCredentials = Object.entries(serverSnapshot.server.git_credentials).map(
      ([host, config]) => ({
        host,
        provider: config.provider,
        username: config.username ?? "",
        detail:
          config.provider === "env"
            ? (config as { token_env: string }).token_env
            : config.provider === "secret"
              ? (config as { secret_id: string }).secret_id
              : "",
      }),
    );
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
      nodeSnapshot = isServer
        ? await sync.connection.client.updateNodeSettingsOf(targetNode, request)
        : await sync.connection.client.updateNodeSettings(request);
      syncNodeForm();
      toast.success("Saved node settings.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function saveServer(): Promise<void> {
    try {
      serverSnapshot = await sync.connection.client.updateServerSettings({
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
    if (secretForm.value === "" || secretForm.name.trim() === "") return;
    const id =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `secret-${Date.now()}`;
    try {
      const request = {
        name: secretForm.name.trim(),
        kind: secretForm.kind,
        value: secretForm.value,
      };
      const response =
        isServer && section === "machine" && targetNode !== ""
          ? await sync.connection.client.upsertNodeSecret(targetNode, id, request)
          : await sync.connection.client.upsertSecret(id, request);
      secrets = response.secrets;
      secretForm = { name: "", kind: secretForm.kind, value: "" };
      toast.success("Secret saved (the value is never shown again).");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function rotateSecret(secret: SecretSummary): Promise<void> {
    try {
      const response =
        isServer && section === "machine" && targetNode !== ""
          ? await sync.connection.client.rotateNodeSecret(targetNode, secret.id)
          : await sync.connection.client.rotateSecret(secret.id);
      secrets = response.secrets;
      toast.success(`Rotated ${secret.name}.`);
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function deleteSecret(secret: SecretSummary): Promise<void> {
    if (!confirm(`Delete secret ${secret.name}?`)) return;
    try {
      const response =
        isServer && section === "machine" && targetNode !== ""
          ? await sync.connection.client.deleteNodeSecret(targetNode, secret.id)
          : await sync.connection.client.deleteSecret(secret.id);
      secrets = response.secrets;
      toast.success(`Deleted ${secret.name}.`);
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
        ? serverSnapshot
          ? [
              {
                label: "Data directory",
                value: serverSnapshot.bootstrap.fxg_home.value,
                source: serverSnapshot.bootstrap.fxg_home.source,
              },
              {
                label: "Listen address",
                value: serverSnapshot.bootstrap.listen_addr.value,
                source: serverSnapshot.bootstrap.listen_addr.source,
              },
              {
                label: "Allowed hosts",
                value: serverSnapshot.bootstrap.allowed_hosts.value.join(", ") || "-",
                source: serverSnapshot.bootstrap.allowed_hosts.source,
              },
              {
                label: "Allowed origins",
                value: serverSnapshot.bootstrap.allowed_origins.value.join(", ") || "-",
                source: serverSnapshot.bootstrap.allowed_origins.source,
              },
              {
                label: "Log level",
                value: serverSnapshot.bootstrap.log_level.value,
                source: serverSnapshot.bootstrap.log_level.source,
              },
              {
                label: "Log file",
                value: serverSnapshot.bootstrap.log_file.value ?? "-",
                source: serverSnapshot.bootstrap.log_file.source,
              },
              {
                label: "Log format",
                value: serverSnapshot.bootstrap.log_format.value,
                source: serverSnapshot.bootstrap.log_format.source,
              },
              {
                label: "No color",
                value: String(serverSnapshot.bootstrap.log_no_color.value),
                source: serverSnapshot.bootstrap.log_no_color.source,
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
                label: "Central server",
                value: nodeSnapshot.bootstrap.central_server_url.value ?? "-",
                source: nodeSnapshot.bootstrap.central_server_url.source,
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
        class="border-input bg-background h-8 rounded-md border px-2 text-xs"
        bind:value={selectedNode}
      >
        {#each onlineNodes as node (node.node_id)}
          <option value={node.node_id}>{node.name} ({node.node_id})</option>
        {/each}
      </select>
      {#if selectedNode === ""}
        <span class="text-muted-foreground text-xs">
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
      <p class="text-muted-foreground text-xs">
        Process start-up values from config.toml, environment variables and CLI flags. Change them
        in the file and restart the process.
      </p>
      {#each filteredBootstrap as row (row.label)}
        <div class="flex flex-wrap items-center gap-2 border-b py-1.5 last:border-b-0">
          <span class="w-40 text-xs font-medium">{row.label}</span>
          <code class="min-w-0 flex-1 truncate text-xs">{row.value}</code>
          <Badge variant="outline" class="text-[10px]">{sourceBadge(row.source)}</Badge>
        </div>
      {/each}
      {#if filteredBootstrap.length === 0}
        <p class="text-muted-foreground text-xs">{loading ? "Loading…" : "No matching values."}</p>
      {/if}
    </Card>
  {:else if section === "machine"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">This machine</h2>
      <p class="text-muted-foreground text-xs">
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
              class="border-input bg-background min-h-24 rounded-md border p-2 font-mono text-xs"
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
          <span class="text-muted-foreground text-xs">revision {nodeForm.revision}</span>
        </div>
      {:else}
        <p class="text-muted-foreground text-xs">
          {loading ? "Loading…" : "Node settings are unavailable."}
        </p>
      {/if}
    </Card>
  {:else if section === "server"}
    {#if !isServer}
      <Card class="gap-2 p-4">
        <h2 class="font-medium">Server settings</h2>
        <p class="text-muted-foreground text-xs">
          Server-owned settings live in the hub's <code>server.db</code>. Open the hub Web UI at
          <a class="underline" href={sync.connection.origin} target="_blank"
            >{sync.connection.origin}</a
          >.
        </p>
      </Card>
    {:else}
      <Card class="gap-3 p-4">
        <h2 class="font-medium">Server</h2>
        <p class="text-muted-foreground text-xs">
          Owned by <code>server.db</code>: Web Push contact and Git credential providers.
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
                placeholder={entry.provider === "env" ? "TOKEN_ENV" : "secret ID"}
                bind:value={entry.detail}
                disabled={entry.provider === "gh_cli"}
              />
            </div>
          {/each}
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
          <span class="text-muted-foreground text-xs">revision {serverForm.revision}</span>
        </div>
      </Card>
    {/if}
  {:else if section === "projects"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">Project profiles</h2>
      {#if !isServer}
        <p class="text-muted-foreground text-xs">
          Bootstrap profiles for temporary VMs are owned by the hub. Open the hub Web UI to edit
          them.
        </p>
      {:else}
        <p class="text-muted-foreground text-xs">
          Setup commands and environment variables injected into temporary VMs (<code
            >project_profiles</code
          >
          in <code>server.db</code>).
        </p>
        {#each sync.projects as project (project.project_id)}
          {@const profile = profiles[project.project_id]}
          <div class="rounded-md border p-2">
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-medium">{project.name}</span>
              <code class="text-muted-foreground text-[11px]">{project.project_id}</code>
              <Badge variant="outline" class="text-[10px]">{project.identity.kind}</Badge>
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
                    class="border-input bg-background min-h-20 rounded-md border p-2 font-mono text-xs"
                    bind:value={profileForm.setup}></textarea>
                </div>
                <div class="grid gap-1.5">
                  <Label for={`env-${project.project_id}`}>Environment (KEY=VALUE)</Label>
                  <textarea
                    id={`env-${project.project_id}`}
                    class="border-input bg-background min-h-20 rounded-md border p-2 font-mono text-xs"
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
              <p class="text-muted-foreground mt-1 text-xs">
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

  {#if section === "bootstrap" || section === "machine" || section === "server"}
    <Card class="gap-3 p-4">
      <h2 class="font-medium">Secrets</h2>
      <p class="text-muted-foreground text-xs">
        Values are write-only: they are stored in <code>~/.flexagent/secrets/</code> and never returned
        by the API.
      </p>
      <ul class="flex list-none flex-col gap-1 p-0 text-xs">
        {#each secrets as secret (secret.id)}
          <li class="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5">
            <span class="font-medium">{secret.name}</span>
            <code class="text-muted-foreground">{secret.kind}</code>
            <Badge variant={secret.configured ? "secondary" : "outline"} class="text-[10px]">
              {secret.configured ? "Configured" : "Missing"}
            </Badge>
            {#if secret.used_by.length > 0}
              <span class="text-muted-foreground">used by {secret.used_by.join(", ")}</span>
            {/if}
            <div class="ml-auto flex gap-1">
              <Button
                variant="ghost"
                size="sm"
                onclick={() => void rotateSecret(secret)}
                disabled={!secret.configured}
              >
                Rotate
              </Button>
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
      <form class="grid gap-2 sm:grid-cols-4" onsubmit={saveSecret}>
        <Input placeholder="Name" bind:value={secretForm.name} />
        <select
          class="border-input bg-background h-9 rounded-md border px-2 text-sm"
          bind:value={secretForm.kind}
        >
          <option value="agent_env">Agent env</option>
          <option value="git_token">Git token</option>
          <option value="node_token">Node token</option>
          <option value="auth_token">Auth token</option>
          <option value="vapid_private">VAPID private key</option>
        </select>
        <Input type="password" placeholder="Value (write-only)" bind:value={secretForm.value} />
        <Button type="submit" disabled={secretForm.name.trim() === "" || secretForm.value === ""}>
          Store secret
        </Button>
      </form>
    </Card>
  {/if}
</div>
