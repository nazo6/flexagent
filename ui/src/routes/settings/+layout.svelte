<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import * as Select from "#lib/components/ui/select/index.ts";
  import { tabsListVariants } from "#lib/components/ui/tabs/tabs-list.svelte";
  import { tabsTriggerClass } from "#lib/components/ui/tabs/tabs-trigger.svelte";
  import { m } from "#lib/paraglide/messages.js";
  import {
    browserSettingsPath,
    defaultTabForScope,
    hubSettingsPath,
    isBrowserTab,
    isHubTab,
    isNodeTab,
    isNodeTabAvailable,
    nodeSettingsPath,
    resolveDefaultNodeId,
    resolveSettingsScope,
    settingsPathForTarget,
    type BrowserSettingsTab,
    type HubSettingsTab,
    type NodeSettingsTab,
    type SettingsScope,
    type SettingsTabId,
  } from "#lib/settings-routes.ts";
  import {
    searchSettings,
    SETTINGS_SCOPE_LABELS,
    SETTINGS_TAB_LABELS,
    type SettingsSearchEntry,
  } from "#lib/settings-search.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import CpuIcon from "@lucide/svelte/icons/cpu";
  import PaletteIcon from "@lucide/svelte/icons/palette";
  import SearchIcon from "@lucide/svelte/icons/search";
  import ServerIcon from "@lucide/svelte/icons/server";

  let { children } = $props();

  const isHub = $derived(sync.connection.role === "hub");
  const onlineNodes = $derived(sync.nodes.filter((node) => node.is_online));
  const defaultNodeId = $derived(
    resolveDefaultNodeId(isHub, sync.nodes, sync.connection.systemInfo?.node_id),
  );

  const activeScope = $derived.by<SettingsScope>(() => {
    const routeId = page.route.id ?? "";
    if (routeId.startsWith("/settings/hub")) return "hub";
    if (routeId.startsWith("/settings/nodes")) return "node";
    if (routeId.startsWith("/settings/browser")) return "browser";
    return isHub ? "hub" : "node";
  });

  const routeNodeId = $derived(page.params.node_id ?? "");
  const activeTab = $derived<SettingsTabId>(
    (page.params.tab as SettingsTabId | undefined) ?? defaultTabForScope(activeScope),
  );

  type TabDef = { id: SettingsTabId; label: string };

  const tabsByScope = $derived<Record<SettingsScope, TabDef[]>>({
    hub: [
      { id: "general", label: m.settings_tab_general() },
      { id: "process", label: m.settings_tab_process() },
      { id: "tokens", label: m.settings_tab_tokens() },
      { id: "provisioners", label: m.settings_tab_provisioners() },
      { id: "audit", label: m.settings_tab_audit() },
    ],
    node: [
      { id: "general", label: m.settings_tab_general() },
      ...(isHub ? [] : [{ id: "process" as const, label: m.settings_tab_process() }]),
      { id: "pairing", label: m.settings_tab_pairing() },
      { id: "agents", label: m.settings_tab_agents() },
      ...(isHub ? [] : [{ id: "opencode" as const, label: m.settings_tab_opencode() }]),
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
      href: string;
    }[]
  >([
    {
      id: "hub",
      label: m.settings_scope_hub(),
      description: isHub ? m.settings_scope_hub_desc() : m.settings_scope_hub_unavailable(),
      icon: ServerIcon,
      disabled: !isHub,
      href: hubSettingsPath("general"),
    },
    {
      id: "node",
      label: m.settings_scope_node(),
      description: isHub ? m.settings_scope_node_desc_hub() : m.settings_scope_node_desc(),
      icon: CpuIcon,
      disabled: false,
      href: nodeSettingsPath("general", defaultNodeId),
    },
    {
      id: "browser",
      label: m.settings_scope_browser(),
      description: m.settings_scope_browser_desc(),
      icon: PaletteIcon,
      disabled: false,
      href: browserSettingsPath("appearance"),
    },
  ]);

  type NodeStatus =
    | { kind: "ok" }
    | { kind: "index" }
    | { kind: "no_online_nodes" }
    | { kind: "offline"; nodeName: string }
    | { kind: "not_found"; nodeId: string };

  const nodeStatus = $derived.by<NodeStatus>(() => {
    if (activeScope !== "node") return { kind: "ok" };
    if (routeNodeId === "") {
      if (isHub && onlineNodes.length === 0) return { kind: "no_online_nodes" };
      return { kind: "index" };
    }
    if (!isHub) {
      const ownId = sync.connection.systemInfo?.node_id ?? "";
      if (routeNodeId === ownId) return { kind: "ok" };
      return { kind: "not_found", nodeId: routeNodeId };
    }
    if (onlineNodes.length === 0) return { kind: "no_online_nodes" };
    if (onlineNodes.some((node) => node.node_id === routeNodeId)) return { kind: "ok" };
    const known = sync.nodes.find((node) => node.node_id === routeNodeId);
    if (known) return { kind: "offline", nodeName: known.name };
    return { kind: "not_found", nodeId: routeNodeId };
  });

  const selectedOnlineNodeId = $derived(
    onlineNodes.some((node) => node.node_id === routeNodeId) ? routeNodeId : "",
  );

  const selectedNodeLabel = $derived.by(() => {
    const node = onlineNodes.find((candidate) => candidate.node_id === routeNodeId);
    if (node) return `${node.name} (${node.node_id})`;
    return routeNodeId !== "" ? routeNodeId : m.settings_target_node();
  });

  function onSelectNode(nextNodeId: string): void {
    if (!nextNodeId || nextNodeId === routeNodeId) return;
    const nextTab: NodeSettingsTab =
      isNodeTab(activeTab) && isNodeTabAvailable(activeTab, isHub) ? activeTab : "general";
    void goto(nodeSettingsPath(nextTab, nextNodeId));
  }

  function tabHref(tabId: SettingsTabId): string {
    switch (activeScope) {
      case "hub":
        return hubSettingsPath(isHubTab(tabId) ? (tabId as HubSettingsTab) : "general");
      case "node":
        return nodeSettingsPath(
          isNodeTab(tabId) ? (tabId as NodeSettingsTab) : "general",
          routeNodeId,
        );
      case "browser":
        return browserSettingsPath(
          isBrowserTab(tabId) ? (tabId as BrowserSettingsTab) : "appearance",
        );
    }
  }

  /** Search query: while set, the scope/tabs are replaced by cross-tab search results. */
  let query = $state("");
  const searchResults = $derived(searchSettings(query));
  /** Search hits available for the current role (hub-only items are hidden on a node). */
  const visibleSearchResults = $derived(
    searchResults.filter((entry) => entry.scope !== "hub" || isHub),
  );

  function searchResultHref(entry: SettingsSearchEntry): string {
    const targetNode =
      activeScope === "node" && nodeStatus.kind === "ok" && routeNodeId !== ""
        ? routeNodeId
        : defaultNodeId;
    return settingsPathForTarget(entry.scope, entry.tab, { isHub, nodeId: targetNode });
  }

  function searchResultTarget(entry: SettingsSearchEntry): string {
    const scope = SETTINGS_SCOPE_LABELS[resolveSettingsScope(entry.scope, isHub)];
    return `${scope} · ${SETTINGS_TAB_LABELS[entry.tab]}`;
  }

  const fallbackNodeTab: NodeSettingsTab = $derived(
    isNodeTab(activeTab) && isNodeTabAvailable(activeTab, isHub) ? activeTab : "general",
  );
  const isIndexRedirectRoute = $derived(
    page.route.id === "/settings" ||
      page.route.id === "/settings/hub" ||
      page.route.id === "/settings/nodes/[node_id]" ||
      page.route.id === "/settings/browser",
  );
</script>

<svelte:head>
  <title>{m.settings_page_title()}</title>
</svelte:head>

{#if page.error || isIndexRedirectRoute}
  {@render children()}
{:else}
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
                <a
                  href={searchResultHref(result)}
                  onclick={() => (query = "")}
                  class="hover:bg-accent flex w-full items-center gap-3 px-3 py-2 text-left transition-colors"
                >
                  <span class="text-[13px]">{result.label}</span>
                  <span class="text-muted-foreground ml-auto text-[11px]">
                    {searchResultTarget(result)}
                  </span>
                </a>
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
          {@const isSelected = activeScope === option.id}
          {#if option.disabled}
            <button
              type="button"
              disabled
              aria-current={isSelected ? "page" : undefined}
              class="border-border flex cursor-not-allowed items-start gap-2 rounded-lg border px-3 py-2.5 text-left opacity-50"
            >
              <ScopeIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
              <span class="flex min-w-0 flex-col">
                <span class="text-[13px] font-semibold">{option.label}</span>
                <span class="text-muted-foreground text-[11px] leading-snug">
                  {option.description}
                </span>
              </span>
            </button>
          {:else}
            <a
              href={option.href}
              aria-current={isSelected ? "page" : undefined}
              class={cn(
                "flex items-start gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
                isSelected
                  ? "border-signal bg-signal-soft/60"
                  : "border-border hover:border-line-strong hover:bg-muted/50",
              )}
            >
              <ScopeIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
              <span class="flex min-w-0 flex-col">
                <span class="text-[13px] font-semibold">{option.label}</span>
                <span class="text-muted-foreground text-[11px] leading-snug">
                  {option.description}
                </span>
              </span>
            </a>
          {/if}
        {/each}
      </div>

      {#if activeScope === "node" && isHub && onlineNodes.length > 0}
        <div
          class="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2"
        >
          <CpuIcon class="text-muted-foreground size-4" />
          <Label for="settings-target-node">{m.settings_target_node()}</Label>
          <Select.Root type="single" value={selectedOnlineNodeId} onValueChange={onSelectNode}>
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

      {#if activeScope === "hub" && !isHub}
        <Card class="gap-3 p-5">
          <div class="flex items-start gap-2">
            <ServerIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
            <div class="flex flex-col gap-2">
              <h2 class="font-medium">{m.settings_scope_hub()}</h2>
              <p class="text-muted-foreground text-[13px]">
                {m.settings_scope_hub_unavailable()}
              </p>
              {#if defaultNodeId !== null}
                <div>
                  <Button
                    href={nodeSettingsPath("general", defaultNodeId)}
                    variant="outline"
                    size="sm"
                  >
                    {m.settings_open_node_settings()}
                  </Button>
                </div>
              {/if}
            </div>
          </div>
        </Card>
      {:else if activeScope === "node" && nodeStatus.kind === "no_online_nodes"}
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
      {:else if activeScope === "node" && (nodeStatus.kind === "offline" || nodeStatus.kind === "not_found")}
        <Card class="gap-3 p-5">
          <div class="flex items-start gap-2">
            <CpuIcon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
            <div class="flex flex-col gap-2">
              <h2 class="font-medium">{m.settings_node_unavailable_title()}</h2>
              <p class="text-muted-foreground text-[13px]">
                {nodeStatus.kind === "offline"
                  ? m.settings_node_offline_description({ name: nodeStatus.nodeName })
                  : m.settings_node_not_found_description({ id: nodeStatus.nodeId })}
              </p>
              {#if defaultNodeId !== null}
                <div>
                  <Button
                    href={nodeSettingsPath(fallbackNodeTab, defaultNodeId)}
                    variant="outline"
                    size="sm"
                  >
                    {m.settings_open_default_node()}
                  </Button>
                </div>
              {/if}
            </div>
          </div>
        </Card>
      {:else if activeScope === "node" && nodeStatus.kind === "index"}
        {@render children()}
      {:else}
        <nav
          aria-label={m.settings_title()}
          data-slot="tabs"
          data-orientation="horizontal"
          data-horizontal=""
          class="group/tabs flex flex-col gap-2"
        >
          <div
            data-slot="tabs-list"
            data-variant="line"
            class={cn(
              tabsListVariants({ variant: "line" }),
              "w-full flex-wrap border-b sm:flex-nowrap",
            )}
          >
            {#each tabsByScope[activeScope] as tab (tab.id)}
              {@const isCurrentTab = activeTab === tab.id}
              <a
                href={tabHref(tab.id)}
                data-slot="tabs-trigger"
                data-active={isCurrentTab ? "" : undefined}
                aria-current={isCurrentTab ? "page" : undefined}
                class={tabsTriggerClass}
              >
                {tab.label}
              </a>
            {/each}
          </div>

          <div class="mt-2 flex flex-col gap-3">
            {@render children()}
          </div>
        </nav>
      {/if}
    {/if}
  </div>
{/if}
