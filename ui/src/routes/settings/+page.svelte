<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import AgentManager from "#lib/components/settings/AgentManager.svelte";
  import AuditLog from "#lib/components/settings/AuditLog.svelte";
  import DiagnosticsPanel from "#lib/components/settings/DiagnosticsPanel.svelte";
  import KillSwitchButton from "#lib/components/KillSwitchButton.svelte";
  import NotificationsPanel from "#lib/components/settings/NotificationsPanel.svelte";
  import PairingPanel from "#lib/components/settings/PairingPanel.svelte";
  import ProvisionerManager from "#lib/components/settings/ProvisionerManager.svelte";
  import SettingsCatalog from "#lib/components/settings/SettingsCatalog.svelte";
  import {
    searchSettings,
    SETTINGS_SCOPE_LABELS,
    SETTINGS_TAB_LABELS,
    type SettingsScope,
    type SettingsSearchEntry,
    type SettingsTabId,
  } from "#lib/settings-search.ts";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import * as Select from "#lib/components/ui/select/index.ts";
  import * as Tabs from "#lib/components/ui/tabs/index.ts";
  import type { IssueNodeTokenResponse, NodeTokenSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { pinned } from "#lib/stores/pinned.svelte.ts";
  import { prefs } from "#lib/stores/prefs.svelte.ts";
  import { createResource } from "#lib/stores/resource.svelte.ts";
  import { PALETTES, theme, type PaletteId } from "#lib/theme.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { setMode, userPrefersMode } from "mode-watcher";
  import { toast } from "svelte-sonner";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import KeyRoundIcon from "@lucide/svelte/icons/key-round";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import SearchIcon from "@lucide/svelte/icons/search";
  import ShieldIcon from "@lucide/svelte/icons/shield";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
  import CpuIcon from "@lucide/svelte/icons/cpu";
  import OctagonXIcon from "@lucide/svelte/icons/octagon-x";
  import PaletteIcon from "@lucide/svelte/icons/palette";
  import ServerIcon from "@lucide/svelte/icons/server";

  const isHub = $derived(sync.connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  /** Node settings need a target; through the hub that means an online node. */
  const nodeScopeAvailable = $derived(!isHub || onlineNodes.length > 0);

  /**
   * Settings scopes: the hub (server), a node (machine), and this browser.
   * Each scope owns its tabs so it is always clear what a setting applies to.
   */
  type TabDef = { id: SettingsTabId; label: string };

  /** Old flat tab ids (deep links, search results from older builds). */
  const LEGACY_TABS: Record<string, { scope: SettingsScope | "current"; tab: SettingsTabId }> = {
    bootstrap: { scope: "current", tab: "process" },
    machine: { scope: "node", tab: "general" },
    hub: { scope: "hub", tab: "general" },
    pairing: { scope: "node", tab: "pairing" },
    agents: { scope: "node", tab: "agents" },
    provisioners: { scope: "hub", tab: "provisioners" },
    browser: { scope: "browser", tab: "appearance" },
    audit: { scope: "current", tab: "audit" },
  };
  const TAB_IDS: SettingsTabId[] = [
    "process",
    "general",
    "pairing",
    "tokens",
    "provisioners",
    "agents",
    "audit",
    "appearance",
    "notifications",
  ];

  function resolveScope(ref: SettingsScope | "current"): SettingsScope {
    return ref === "current" ? (isHub ? "hub" : "node") : ref;
  }

  const tabsByScope = $derived<Record<SettingsScope, TabDef[]>>({
    hub: [
      { id: "process", label: m.settings_tab_process() },
      { id: "general", label: m.settings_tab_general() },
      { id: "tokens", label: m.settings_tab_tokens() },
      { id: "provisioners", label: m.settings_tab_provisioners() },
      { id: "audit", label: m.settings_tab_audit() },
    ],
    node: [
      ...(isHub ? [] : [{ id: "process" as const, label: m.settings_tab_process() }]),
      { id: "general", label: m.settings_tab_general() },
      { id: "pairing", label: m.settings_tab_pairing() },
      { id: "agents", label: m.settings_tab_agents() },
      ...(isHub ? [] : [{ id: "audit" as const, label: m.settings_tab_audit() }]),
    ],
    browser: [
      { id: "appearance", label: m.settings_tab_appearance() },
      { id: "notifications", label: m.settings_tab_notifications() },
    ],
  });

  const scopeOptions = $derived<
    {
      id: SettingsScope;
      label: string;
      description: string;
      icon: typeof ServerIcon;
      disabled: boolean;
    }[]
  >([
    {
      id: "hub",
      label: m.settings_scope_hub(),
      description: isHub ? m.settings_scope_hub_desc() : m.settings_scope_hub_unavailable(),
      icon: ServerIcon,
      disabled: !isHub,
    },
    {
      id: "node",
      label: m.settings_scope_node(),
      description: isHub ? m.settings_scope_node_desc_hub() : m.settings_scope_node_desc(),
      icon: CpuIcon,
      disabled: false,
    },
    {
      id: "browser",
      label: m.settings_scope_browser(),
      description: m.settings_scope_browser_desc(),
      icon: PaletteIcon,
      disabled: false,
    },
  ]);

  // The initial location is read once; later role changes are handled by the effect below.
  const initialIsHub = untrack(() => isHub);
  const initialTabParam = page.url.searchParams.get("tab");
  const initialScopeParam = page.url.searchParams.get("scope");
  const initialLegacy = initialTabParam ? LEGACY_TABS[initialTabParam] : undefined;
  const initialScope: SettingsScope =
    initialScopeParam === "hub" || initialScopeParam === "node" || initialScopeParam === "browser"
      ? initialScopeParam
      : initialLegacy
        ? resolveScope(initialLegacy.scope)
        : initialIsHub
          ? "hub"
          : "node";
  const initialTab: SettingsTabId = initialLegacy
    ? initialLegacy.tab
    : initialTabParam && TAB_IDS.includes(initialTabParam as SettingsTabId)
      ? (initialTabParam as SettingsTabId)
      : "general";

  /** Scope and tab currently shown (mirrored into `?scope=&tab=` for deep links). */
  let activeScope = $state<SettingsScope>(initialScope);
  let activeTab = $state<SettingsTabId>(initialTab);

  /** Node edited by the Node scope (through the hub or the local node). */
  let selectedNode = $state("");

  $effect(() => {
    if (isHub && selectedNode === "" && onlineNodes.length > 0) {
      selectedNode = onlineNodes[0].node_id;
    }
  });

  /** Node the Node scope writes to (empty while none is online). */
  const targetNode = $derived(isHub ? selectedNode : (sync.connection.systemInfo?.node_id ?? ""));
  /** Label of the selected node in the target dropdown. */
  const selectedNodeLabel = $derived.by(() => {
    const node = onlineNodes.find((candidate) => candidate.node_id === selectedNode);
    return node ? `${node.name} (${node.node_id})` : selectedNode;
  });

  // Keep the location valid when the role or the available tabs change.
  $effect(() => {
    let changed = false;
    if (activeScope === "hub" && !isHub) {
      activeScope = "node";
      changed = true;
    }
    const tabs = tabsByScope[activeScope];
    if (!tabs.some((tab) => tab.id === activeTab)) {
      activeTab = tabs[0]?.id ?? "general";
      changed = true;
    }
    if (changed) writeUrl();
  });

  function writeUrl(): void {
    const url = new URL(location.href);
    if (
      url.searchParams.get("scope") === activeScope &&
      url.searchParams.get("tab") === activeTab
    ) {
      return;
    }
    url.searchParams.set("scope", activeScope);
    url.searchParams.set("tab", activeTab);
    void goto(url, { shallow: true, replace: true });
  }

  function selectScope(scope: SettingsScope): void {
    if (scope === activeScope) return;
    activeScope = scope;
    const tabs = tabsByScope[scope];
    if (!tabs.some((tab) => tab.id === activeTab)) {
      activeTab = tabs[0]?.id ?? "general";
    }
    writeUrl();
  }

  function selectTab(value: string): void {
    activeTab = value as SettingsTabId;
    writeUrl();
  }

  function selectSearchResult(entry: SettingsSearchEntry): void {
    activeScope = resolveScope(entry.scope);
    activeTab = entry.tab;
    if (!tabsByScope[activeScope].some((tab) => tab.id === activeTab)) {
      activeTab = tabsByScope[activeScope][0]?.id ?? "general";
    }
    writeUrl();
    query = "";
  }

  function searchResultTarget(entry: SettingsSearchEntry): string {
    const scope = SETTINGS_SCOPE_LABELS[resolveScope(entry.scope)];
    return `${scope} · ${SETTINGS_TAB_LABELS[entry.tab]}`;
  }

  /** Theme preference stored in this browser (`system` / `light` / `dark`). */
  const themePreference = $derived(userPrefersMode.current);

  function themeModeLabel(value: string): string {
    switch (value) {
      case "light":
        return m.settings_browser_theme_light();
      case "dark":
        return m.settings_browser_theme_dark();
      default:
        return m.settings_browser_theme_system();
    }
  }

  function shortcutLabel(value: string): string {
    return value === "modifier_enter"
      ? m.settings_browser_send_shortcut_modifier()
      : m.settings_browser_send_shortcut_enter();
  }

  function paletteLabel(id: PaletteId): string {
    switch (id) {
      case "bone":
        return m.settings_appearance_palette_bone();
      case "graphite":
        return m.settings_appearance_palette_graphite();
      case "moss":
        return m.settings_appearance_palette_moss();
    }
  }
  /** Search query: while set, the tabs are replaced by cross-tab search results. */
  let query = $state("");
  const searchResults = $derived(searchSettings(query));
  /** Search hits available for the current role (hub-only items are hidden on a node). */
  const visibleSearchResults = $derived(
    searchResults.filter((entry) => entry.scope !== "hub" || isHub),
  );

  // --- Node pairing (hub only) ---
  const tokens = createResource<NodeTokenSummary[]>(() => sync.connection.client.nodeTokens(), {
    onError: (error) => toast.error(errorMessage(error)),
  });

  // Issue a new node pairing token
  let issueOpen = $state(false);
  let issueNodeId = $state("");
  let issueBusy = $state(false);
  /** Plaintext token right after issuance (cannot be shown again once this dialog is closed). */
  let issued = $state<IssueNodeTokenResponse | null>(null);
  /** Copy-paste pairing command for the node (uses this hub's HTTP origin). */
  const pairCommand = $derived(
    issued === null
      ? ""
      : `fxg node pair --hub ${typeof location === "undefined" ? "" : location.origin} --token ${issued.token}`,
  );

  // Token revocation confirmation
  let revokeTarget = $state<string | null>(null);
  let revokeBusy = $state(false);

  // --- Web access token ---
  let rotateOpen = $state(false);
  let rotateBusy = $state(false);

  onMount(() => {
    if (isHub) void tokens.refresh();
  });

  async function submitIssue(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (issueBusy || issueNodeId.trim() === "") return;
    issueBusy = true;
    try {
      issued = await sync.connection.client.issueNodeToken({
        node_id: issueNodeId.trim(),
      });
      issueOpen = false;
      issueNodeId = "";
      await tokens.refresh();
      await sync.refreshNodes();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      issueBusy = false;
    }
  }

  async function confirmRevoke(): Promise<void> {
    const nodeId = revokeTarget;
    if (nodeId === null || revokeBusy) return;
    revokeBusy = true;
    try {
      await sync.connection.client.revokeNodeToken(nodeId);
      toast.success(m.settings_node_token_revoked({ name: nodeId }));
      revokeTarget = null;
      await tokens.refresh();
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      revokeBusy = false;
    }
  }

  async function copyText(text: string, label: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(m.settings_copied({ name: label }));
    } catch {
      toast.error(m.settings_copy_failed());
    }
  }

  async function confirmRotate(): Promise<void> {
    if (rotateBusy) return;
    rotateBusy = true;
    try {
      const result = await sync.connection.client.rotateAuthToken();
      // The old token is invalidated immediately, so re-authenticate with the new token right away.
      await sync.connection.login(result.token);
      sync.restart();
      toast.success(m.settings_token_regenerated());
      rotateOpen = false;
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      rotateBusy = false;
    }
  }
</script>

<svelte:head>
  <title>{m.settings_page_title()}</title>
</svelte:head>

<div class="desk-page">
  <div class="flex flex-col gap-1">
    <h1 class="text-xl font-semibold tracking-tight">{m.settings_title()}</h1>
    <p class="text-muted-foreground text-[13px]">
      {m.settings_subtitle()}
    </p>
  </div>

  <div class="relative w-full max-w-md">
    <SearchIcon
      class="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4"
    />
    <Input
      placeholder={m.settings_search_placeholder()}
      aria-label={m.settings_search_placeholder()}
      bind:value={query}
      class="pl-9"
    />
  </div>

  {#if query.trim() !== ""}
    <section class="flex flex-col gap-2">
      <h2 class="desk-label">{m.settings_search_results({ query: query.trim() })}</h2>
      {#if visibleSearchResults.length === 0}
        <p class="text-muted-foreground text-[13px]">
          {m.settings_search_empty({ query: query.trim() })}
        </p>
      {:else}
        <ul class="divide-y overflow-hidden rounded-lg border">
          {#each visibleSearchResults as result (result.scope + ":" + result.tab + ":" + result.label)}
            <li>
              <button
                type="button"
                class="hover:bg-accent flex w-full items-center gap-3 px-3 py-2 text-left transition-colors"
                onclick={() => selectSearchResult(result)}
              >
                <span class="text-[13px]">{result.label}</span>
                <span class="text-muted-foreground ml-auto text-[11px]">
                  {searchResultTarget(result)}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <!-- Scope switch: Hub (server) / Node (machine) / Browser (this device) -->
    <div class="grid gap-2 sm:grid-cols-3">
      {#each scopeOptions as option (option.id)}
        {@const ScopeIcon = option.icon}
        <button
          type="button"
          aria-pressed={activeScope === option.id}
          disabled={option.disabled}
          onclick={() => selectScope(option.id)}
          class={cn(
            "flex items-start gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
            activeScope === option.id
              ? "border-signal bg-signal-soft/60"
              : "border-border hover:border-line-strong hover:bg-muted/50",
            option.disabled &&
              "cursor-not-allowed opacity-50 hover:border-border hover:bg-transparent",
          )}
        >
          <ScopeIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <span class="flex min-w-0 flex-col">
            <span class="text-[13px] font-semibold">{option.label}</span>
            <span class="text-muted-foreground text-[11px] leading-snug">
              {option.description}
            </span>
          </span>
        </button>
      {/each}
    </div>

    {#if activeScope === "node" && isHub && nodeScopeAvailable}
      <div
        class="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2"
      >
        <CpuIcon class="text-muted-foreground size-4" />
        <Label for="settings-target-node">{m.settings_target_node()}</Label>
        <Select.Root type="single" bind:value={selectedNode}>
          <Select.Trigger
            id="settings-target-node"
            size="sm"
            class="min-w-52 text-[13px]"
            aria-label={m.settings_target_node()}
          >
            <span class="truncate">{selectedNodeLabel}</span>
          </Select.Trigger>
          <Select.Content>
            {#each onlineNodes as node (node.node_id)}
              <Select.Item
                value={node.node_id}
                label={`${node.name} (${node.node_id})`}
                class="text-[13px]"
              >
                {node.name} ({node.node_id})
              </Select.Item>
            {/each}
          </Select.Content>
        </Select.Root>
        <span class="text-muted-foreground text-[12px]">{m.settings_node_scope_hint()}</span>
      </div>
    {/if}

    {#if activeScope === "node" && !nodeScopeAvailable}
      <Card class="gap-3 p-5">
        <div class="flex items-start gap-2">
          <CpuIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
          <div class="flex flex-col gap-1">
            <h2 class="font-medium">{m.settings_node_empty_title()}</h2>
            <p class="text-muted-foreground text-[13px]">
              {m.settings_node_empty_description()}
            </p>
          </div>
        </div>
      </Card>
    {:else}
      <Tabs.Root value={activeTab} onValueChange={selectTab}>
        <Tabs.List variant="line" class="w-full flex-wrap border-b sm:flex-nowrap">
          {#each tabsByScope[activeScope] as tab (tab.id)}
            <Tabs.Trigger value={tab.id}>{tab.label}</Tabs.Trigger>
          {/each}
        </Tabs.List>

        {#if activeScope === "hub"}
          <Tabs.Content value="process" class="mt-2 flex flex-col gap-3">
            <SettingsCatalog section="bootstrap" />
            <DiagnosticsPanel />
            {@render emergencyStopCard()}
          </Tabs.Content>

          <Tabs.Content value="general" class="mt-2 flex flex-col gap-3">
            <SettingsCatalog section="hub" />
            <Card class="gap-3 p-4">
              <div class="flex items-start gap-2">
                <ShieldIcon class="text-muted-foreground mt-0.5 size-4" />
                <div class="flex flex-1 flex-col gap-1">
                  <h2 class="font-medium">{m.settings_client_token_title()}</h2>
                  <p class="text-muted-foreground text-[13px]">
                    {m.settings_client_token_description()}
                    <code>&lt;data&gt;/flexagent/{isHub ? "hub" : "node"}/auth_token</code>
                    {m.settings_client_token_description_ref()}
                  </p>
                  <div class="mt-1">
                    <Button variant="outline" size="sm" onclick={() => (rotateOpen = true)}>
                      <RefreshCwIcon />
                      {m.settings_regenerate_token()}
                    </Button>
                  </div>
                </div>
              </div>
            </Card>
          </Tabs.Content>

          <Tabs.Content value="tokens" class="mt-2 flex flex-col gap-3">
            <Card class="gap-3 p-4">
              <div class="flex items-start gap-2">
                <KeyRoundIcon class="text-muted-foreground mt-0.5 size-4" />
                <div class="flex flex-1 flex-col gap-2">
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <h2 class="font-medium">{m.settings_node_pairing_title()}</h2>
                      <p class="text-muted-foreground text-[13px]">
                        {m.settings_node_pairing_description()}
                      </p>
                    </div>
                    <div class="flex items-center gap-1.5">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={!isHub || tokens.loading}
                        onclick={() => void tokens.refresh()}
                      >
                        <RefreshCwIcon />
                        {m.common_refresh()}
                      </Button>
                      <Button size="sm" disabled={!isHub} onclick={() => (issueOpen = true)}>
                        {m.settings_issue_token()}
                      </Button>
                    </div>
                  </div>

                  {#if !isHub}
                    <p class="text-muted-foreground text-[13px]">
                      {m.settings_node_pairing_hub_only()}
                      <code>{sync.connection.origin}</code>
                    </p>
                  {:else if (tokens.data ?? []).length === 0}
                    <p class="text-muted-foreground text-[13px]">
                      {tokens.loading ? m.common_loading() : m.settings_no_node_tokens()}
                    </p>
                  {:else}
                    <ul class="flex list-none flex-col gap-1 p-0 text-[13px]">
                      {#each tokens.data ?? [] as token (token.node_id)}
                        <li
                          class="hover:bg-muted/50 flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5"
                        >
                          <span class="font-medium">{token.node_id}</span>
                          <span class="text-muted-foreground font-mono">{token.token_prefix}…</span>
                          <Badge variant="outline" class="text-[11px]"
                            >{m.settings_badge_issued()}</Badge
                          >
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            class="text-muted-foreground hover:text-destructive ml-auto"
                            disabled={revokeBusy}
                            onclick={() => (revokeTarget = token.node_id)}
                            title={m.settings_revoke_token()}
                          >
                            <Trash2Icon />
                            <span class="sr-only">
                              {m.settings_revoke_token_for({ name: token.node_id })}
                            </span>
                          </Button>
                        </li>
                      {/each}
                    </ul>
                  {/if}
                </div>
              </div>
            </Card>
          </Tabs.Content>

          <Tabs.Content value="provisioners" class="mt-2 flex flex-col gap-3">
            <ProvisionerManager />
          </Tabs.Content>

          <Tabs.Content value="audit" class="mt-2 flex flex-col gap-3">
            <AuditLog />
          </Tabs.Content>
        {:else if activeScope === "node"}
          {#if !isHub}
            <Tabs.Content value="process" class="mt-2 flex flex-col gap-3">
              <SettingsCatalog section="bootstrap" />
              <DiagnosticsPanel />
              {@render emergencyStopCard()}
            </Tabs.Content>
          {/if}

          <Tabs.Content value="general" class="mt-2 flex flex-col gap-3">
            <SettingsCatalog section="machine" nodeId={targetNode} />
            {#if !isHub}
              <Card class="gap-3 p-4">
                <div class="flex items-start gap-2">
                  <ShieldIcon class="text-muted-foreground mt-0.5 size-4" />
                  <div class="flex flex-1 flex-col gap-1">
                    <h2 class="font-medium">{m.settings_client_token_title()}</h2>
                    <p class="text-muted-foreground text-[13px]">
                      {m.settings_client_token_description()}
                      <code>&lt;data&gt;/flexagent/{isHub ? "hub" : "node"}/auth_token</code>
                      {m.settings_client_token_description_ref()}
                    </p>
                    <div class="mt-1">
                      <Button variant="outline" size="sm" onclick={() => (rotateOpen = true)}>
                        <RefreshCwIcon />
                        {m.settings_regenerate_token()}
                      </Button>
                    </div>
                  </div>
                </div>
              </Card>
            {/if}
          </Tabs.Content>

          <Tabs.Content value="pairing" class="mt-2 flex flex-col gap-3">
            <PairingPanel nodeId={targetNode} />
          </Tabs.Content>

          <Tabs.Content value="agents" class="mt-2 flex flex-col gap-3">
            <AgentManager nodeId={targetNode} />
          </Tabs.Content>

          {#if !isHub}
            <Tabs.Content value="audit" class="mt-2 flex flex-col gap-3">
              <AuditLog />
            </Tabs.Content>
          {/if}
        {:else}
          <Tabs.Content value="appearance" class="mt-2 flex flex-col gap-3">
            <Card class="gap-4 p-5">
              <div class="flex flex-col gap-1">
                <h2 class="text-sm font-semibold">{m.settings_appearance_theme()}</h2>
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_appearance_theme_note()}
                </p>
              </div>
              <div class="grid max-w-sm gap-1.5">
                <Select.Root
                  type="single"
                  value={themePreference ?? "system"}
                  onValueChange={(value) => setMode(value as "system" | "light" | "dark")}
                >
                  <Select.Trigger
                    id="browser-theme"
                    size="sm"
                    class="w-full text-[13px]"
                    aria-label={m.settings_appearance_theme()}
                  >
                    <span>{themeModeLabel(themePreference ?? "system")}</span>
                  </Select.Trigger>
                  <Select.Content>
                    <Select.Item
                      value="system"
                      label={m.settings_browser_theme_system()}
                      class="text-[13px]"
                    >
                      {m.settings_browser_theme_system()}
                    </Select.Item>
                    <Select.Item
                      value="light"
                      label={m.settings_browser_theme_light()}
                      class="text-[13px]"
                    >
                      {m.settings_browser_theme_light()}
                    </Select.Item>
                    <Select.Item
                      value="dark"
                      label={m.settings_browser_theme_dark()}
                      class="text-[13px]"
                    >
                      {m.settings_browser_theme_dark()}
                    </Select.Item>
                  </Select.Content>
                </Select.Root>
              </div>
            </Card>

            <Card class="gap-4 p-5">
              <div class="flex flex-col gap-1">
                <h2 class="text-sm font-semibold">{m.settings_appearance_palette()}</h2>
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_appearance_palette_note()}
                </p>
              </div>
              <div class="grid gap-2 sm:grid-cols-3">
                {#each PALETTES as option (option.id)}
                  <button
                    type="button"
                    aria-pressed={theme.palette === option.id}
                    onclick={() => theme.setPalette(option.id)}
                    class={cn(
                      "flex items-center justify-between gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
                      theme.palette === option.id
                        ? "border-signal bg-signal-soft/60"
                        : "border-border hover:border-line-strong hover:bg-muted/50",
                    )}
                  >
                    <span class="flex items-center gap-2">
                      <span
                        class="size-3.5 rounded-full border border-black/10"
                        style={`background:${option.canvas}`}
                      ></span>
                      <span
                        class="size-3.5 rounded-full border border-black/10"
                        style={`background:${option.ink}`}
                      ></span>
                      <span
                        class="size-3.5 rounded-full border border-black/10"
                        style={`background:${option.signal}`}
                      ></span>
                    </span>
                    <span class="text-[13px] font-medium">{paletteLabel(option.id)}</span>
                  </button>
                {/each}
              </div>
              <p class="text-muted-foreground text-[13px]">
                {m.settings_appearance_storage_note()}
              </p>
            </Card>

            <Card class="gap-4 p-5">
              <div class="flex flex-col gap-1">
                <h2 class="text-sm font-semibold">{m.settings_browser_send_shortcut()}</h2>
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_browser_send_shortcut_note()}
                </p>
              </div>
              <div class="grid max-w-sm gap-1.5">
                <Select.Root
                  type="single"
                  value={prefs.sendShortcut}
                  onValueChange={(value) =>
                    prefs.setSendShortcut(value as "enter" | "modifier_enter")}
                >
                  <Select.Trigger
                    id="browser-send-shortcut"
                    size="sm"
                    class="w-full text-[13px]"
                    aria-label={m.settings_browser_send_shortcut()}
                  >
                    <span>{shortcutLabel(prefs.sendShortcut)}</span>
                  </Select.Trigger>
                  <Select.Content>
                    <Select.Item
                      value="enter"
                      label={m.settings_browser_send_shortcut_enter()}
                      class="text-[13px]"
                    >
                      {m.settings_browser_send_shortcut_enter()}
                    </Select.Item>
                    <Select.Item
                      value="modifier_enter"
                      label={m.settings_browser_send_shortcut_modifier()}
                      class="text-[13px]"
                    >
                      {m.settings_browser_send_shortcut_modifier()}
                    </Select.Item>
                  </Select.Content>
                </Select.Root>
              </div>
            </Card>

            <Card class="gap-3 p-4">
              <h2 class="text-sm font-semibold">{m.settings_browser_pinned()}</h2>
              <p class="text-muted-foreground text-[13px]">
                {m.settings_browser_pinned_note()}
              </p>
              <div class="flex items-center justify-between gap-3">
                <span class="text-muted-foreground text-[13px]">
                  {m.settings_browser_pinned_count({ count: pinned.pinnedIds.size })}
                </span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={pinned.pinnedIds.size === 0}
                  onclick={() => pinned.clear()}
                >
                  {m.settings_browser_unpin_all()}
                </Button>
              </div>
            </Card>
          </Tabs.Content>

          <Tabs.Content value="notifications" class="mt-2 flex flex-col gap-3">
            <NotificationsPanel
              onOpenVapidSubject={() => {
                activeScope = isHub ? "hub" : "node";
                activeTab = "general";
                writeUrl();
              }}
            />
          </Tabs.Content>
        {/if}
      </Tabs.Root>
    {/if}
  {/if}
</div>

{#snippet emergencyStopCard()}
  <Card class="border-destructive/40 gap-3 p-4">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div class="flex items-start gap-2">
        <OctagonXIcon class="text-destructive mt-0.5 size-4 shrink-0" />
        <div class="flex flex-col gap-1">
          <h2 class="font-medium">{m.settings_emergency_stop_title()}</h2>
          <p class="text-muted-foreground text-[13px]">
            {m.settings_emergency_stop_description()}
          </p>
        </div>
      </div>
      <KillSwitchButton />
    </div>
  </Card>
{/snippet}

<!-- Issue node pairing token -->
<Dialog.Root open={issueOpen} onOpenChange={(open) => (issueOpen = open)}>
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.settings_issue_node_token_title()}</Dialog.Title>
      <Dialog.Description>
        {m.settings_issue_node_token_description()}
        <code>fxg node start</code>
        {m.settings_issue_node_token_description_as()}
        <code>node_id</code>
        {m.settings_issue_node_token_description_note()}
      </Dialog.Description>
    </Dialog.Header>
    <form class="flex flex-col gap-4" onsubmit={submitIssue}>
      <div class="grid gap-2">
        <Label for="node-id">{m.settings_node_id()}</Label>
        <Input
          id="node-id"
          bind:value={issueNodeId}
          placeholder="home-win"
          class="font-mono text-[13px]"
        />
      </div>
      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={() => (issueOpen = false)}>
          {m.common_cancel()}
        </Button>
        <Button type="submit" disabled={issueBusy || issueNodeId.trim() === ""}>
          {issueBusy ? m.settings_issuing() : m.settings_issue()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- Show the issued plaintext token (one time only) -->
<Dialog.Root open={issued !== null} onOpenChange={(open) => !open && (issued = null)}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{m.settings_node_token_issued_title()}</Dialog.Title>
      <Dialog.Description>
        {m.settings_node_token_issued_description()}
      </Dialog.Description>
    </Dialog.Header>
    <div class="flex flex-col gap-3">
      <div class="grid gap-2">
        <Label>{m.settings_node_id()}</Label>
        <code class="bg-muted rounded-md border px-2 py-1 text-[13px]">{issued?.node_id}</code>
      </div>
      <div class="grid gap-2">
        <Label>{m.settings_token()}</Label>
        <div class="flex items-center gap-1.5">
          <code
            class="bg-muted min-w-0 flex-1 rounded-md border px-2 py-1 font-mono text-[13px] break-all"
          >
            {issued?.token ?? ""}
          </code>
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-8 shrink-0 gap-1.5 px-2.5 text-[13px]"
            onclick={() => issued && void copyText(issued.token, m.settings_token())}
          >
            <CopyIcon class="size-3.5" />
            {m.settings_copy()}
          </Button>
        </div>
      </div>
      <div class="grid gap-2">
        <Label>{m.settings_node_pair_command_label()}</Label>
        <div class="flex items-start gap-1.5">
          <code
            class="bg-muted min-w-0 flex-1 rounded-md border px-2 py-1 text-[12px] break-all whitespace-pre-wrap"
          >
            {pairCommand}
          </code>
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="h-8 shrink-0 gap-1.5 px-2.5 text-[13px]"
            onclick={() => void copyText(pairCommand, m.settings_node_pair_command_label())}
          >
            <CopyIcon class="size-3.5" />
            {m.settings_copy()}
          </Button>
        </div>
        <p class="text-muted-foreground text-[12px]">
          {m.settings_node_pair_command_hint()}
        </p>
      </div>
      <details class="text-[13px]">
        <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[12px]">
          {m.settings_node_manual_setup()}
        </summary>
        <p class="text-muted-foreground mt-1 text-[12px]">
          {m.settings_node_manual_setup_hint()}
        </p>
      </details>
    </div>
    <Dialog.Footer>
      <Button
        type="button"
        variant="outline"
        onclick={() => issued && void copyText(issued.token, m.settings_token())}
      >
        <CopyIcon />
        {m.settings_copy_token()}
      </Button>
      <Button type="button" onclick={() => (issued = null)}>{m.common_close()}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- Revoke node pairing token confirmation -->
<AlertDialog.Root
  open={revokeTarget !== null}
  onOpenChange={(open) => !open && (revokeTarget = null)}
>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.settings_revoke_node_token_title()}</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        <code>{revokeTarget}</code>
        {m.settings_revoke_node_token_description()}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={revokeBusy}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={revokeBusy}
        onclick={() => void confirmRevoke()}
      >
        {revokeBusy ? m.settings_revoking() : m.settings_revoke()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<!-- Client token regeneration confirmation -->
<AlertDialog.Root open={rotateOpen} onOpenChange={(open) => (rotateOpen = open)}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.settings_rotate_token_title()}</AlertDialog.Title>
      <AlertDialog.Description class="text-sm">
        {m.settings_rotate_token_description()}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={rotateBusy}>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action disabled={rotateBusy} onclick={() => void confirmRotate()}>
        {rotateBusy ? m.settings_regenerating() : m.settings_regenerate()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
