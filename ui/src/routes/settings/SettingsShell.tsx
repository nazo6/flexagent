import type { JSX } from "@solidjs/web";
import { useLocation, useNavigate, useParams } from "@solidjs/router";
import { createEffect, createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import IconCpu from "~icons/lucide/cpu";
import IconPalette from "~icons/lucide/palette";
import IconSearch from "~icons/lucide/search";
import IconServer from "~icons/lucide/server";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import * as Select from "#lib/components/ui/select/index.ts";
import { tabsListVariants } from "#lib/components/ui/tabs/index.ts";
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
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";

/**
 * Tab triggers are rendered as links (so they navigate like the URL does)
 * instead of Terracotta tabs. The class list mirrors the Svelte
 * `tabsTriggerClass`; the Solid tabs wrapper keeps its trigger class private.
 */
const settingsTabTriggerClass =
  "gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium group-data-[variant=default]/tabs-list:data-active:shadow-sm group-data-[variant=line]/tabs-list:data-active:shadow-none [&_svg:not([class*='size-'])]:size-4 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 relative inline-flex h-[calc(100%_-_1px)] flex-1 items-center justify-center whitespace-nowrap text-foreground/60 transition-all group-data-[orientation=vertical]/tabs:w-full group-data-[orientation=vertical]/tabs:justify-start hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 dark:text-muted-foreground dark:hover:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 group-data-[variant=line]/tabs-list:bg-transparent group-data-[variant=line]/tabs-list:data-active:bg-transparent dark:group-data-[variant=line]/tabs-list:data-active:border-transparent dark:group-data-[variant=line]/tabs-list:data-active:bg-transparent data-active:bg-background data-active:text-foreground dark:data-active:border-input dark:data-active:bg-input/30 dark:data-active:text-foreground after:absolute after:bg-signal after:opacity-0 after:transition-opacity group-data-[orientation=horizontal]/tabs:after:inset-x-0 group-data-[orientation=horizontal]/tabs:after:bottom-[-5px] group-data-[orientation=horizontal]/tabs:after:h-0.5 group-data-[orientation=vertical]/tabs:after:inset-y-0 group-data-[orientation=vertical]/tabs:after:-right-1 group-data-[orientation=vertical]/tabs:after:w-0.5 group-data-[variant=line]/tabs-list:data-active:after:opacity-100";

type TabDef = { id: SettingsTabId; label: string };

type NodeStatus =
  | { kind: "ok" }
  | { kind: "index" }
  | { kind: "no_online_nodes" }
  | { kind: "offline"; nodeName: string }
  | { kind: "not_found"; nodeId: string };

/** Settings shell: owns the scope / tab navigation and renders the route. */
export default function SettingsShell(props: { children?: JSX.Element }) {
  const location = useLocation();
  const params = useParams<{ tab?: string; node_id?: string }>();
  const navigate = useNavigate();

  const isHub = () => sync.connection.role === "hub";
  const onlineNodes = createMemo(() => sync.nodes.filter((node) => node.is_online));
  const defaultNodeId = createMemo(() =>
    resolveDefaultNodeId(isHub(), sync.nodes, sync.connection.systemInfo?.node_id),
  );

  const activeScope = createMemo<SettingsScope>(() => {
    const pathname = location.pathname ?? "";
    if (pathname.startsWith("/settings/hub")) return "hub";
    if (pathname.startsWith("/settings/nodes")) return "node";
    if (pathname.startsWith("/settings/browser")) return "browser";
    return isHub() ? "hub" : "node";
  });

  const routeNodeId = () => params.node_id ?? "";
  const activeTab = createMemo<SettingsTabId>(
    () => (params.tab as SettingsTabId | undefined) ?? defaultTabForScope(activeScope()),
  );

  const tabsByScope = (): Record<SettingsScope, TabDef[]> => ({
    hub: [
      { id: "general", label: m.settings_tab_general() },
      { id: "process", label: m.settings_tab_process() },
      { id: "tokens", label: m.settings_tab_tokens() },
      { id: "provisioners", label: m.settings_tab_provisioners() },
      { id: "audit", label: m.settings_tab_audit() },
    ],
    node: [
      { id: "general", label: m.settings_tab_general() },
      { id: "process", label: m.settings_tab_process() },
      { id: "pairing", label: m.settings_tab_pairing() },
      { id: "agents", label: m.settings_tab_agents() },
      { id: "audit", label: m.settings_tab_audit() },
    ],
    browser: [
      { id: "appearance", label: m.settings_tab_appearance() },
      { id: "notifications", label: m.settings_tab_notifications() },
    ],
  });

  const scopeOptions = createMemo<
    {
      id: SettingsScope;
      label: string;
      description: string;
      icon: (iconProps: { class?: string }) => JSX.Element;
      disabled: boolean;
      href: string;
    }[]
  >(() => [
    {
      id: "hub",
      label: m.settings_scope_hub(),
      description: isHub() ? m.settings_scope_hub_desc() : m.settings_scope_hub_unavailable(),
      icon: IconServer,
      disabled: !isHub(),
      href: hubSettingsPath("general"),
    },
    {
      id: "node",
      label: m.settings_scope_node(),
      description: isHub() ? m.settings_scope_node_desc_hub() : m.settings_scope_node_desc(),
      icon: IconCpu,
      disabled: false,
      href: nodeSettingsPath("general", defaultNodeId()),
    },
    {
      id: "browser",
      label: m.settings_scope_browser(),
      description: m.settings_scope_browser_desc(),
      icon: IconPalette,
      disabled: false,
      href: browserSettingsPath("appearance"),
    },
  ]);

  const nodeStatus = createMemo<NodeStatus>(() => {
    if (activeScope() !== "node") return { kind: "ok" };
    const nodeId = routeNodeId();
    if (nodeId === "") {
      if (isHub() && onlineNodes().length === 0) return { kind: "no_online_nodes" };
      return { kind: "index" };
    }
    if (!isHub()) {
      const ownId = sync.connection.systemInfo?.node_id ?? "";
      if (nodeId === ownId) return { kind: "ok" };
      return { kind: "not_found", nodeId };
    }
    if (onlineNodes().length === 0) return { kind: "no_online_nodes" };
    if (onlineNodes().some((node) => node.node_id === nodeId)) return { kind: "ok" };
    const known = sync.nodes.find((node) => node.node_id === nodeId);
    if (known) return { kind: "offline", nodeName: known.name };
    return { kind: "not_found", nodeId };
  });

  const selectedOnlineNodeId = (): string =>
    onlineNodes().some((node) => node.node_id === routeNodeId()) ? routeNodeId() : "";

  const selectedNodeLabel = createMemo(() => {
    const node = onlineNodes().find((candidate) => candidate.node_id === routeNodeId());
    if (node) return `${node.name} (${node.node_id})`;
    return routeNodeId() !== "" ? routeNodeId() : m.settings_target_node();
  });

  const unavailableMessage = createMemo(() => {
    const status = nodeStatus();
    if (status.kind === "offline")
      return m.settings_node_offline_description({ name: status.nodeName });
    if (status.kind === "not_found")
      return m.settings_node_not_found_description({ id: status.nodeId });
    return "";
  });

  function onSelectNode(nextNodeId: string): void {
    if (nextNodeId === "" || nextNodeId === routeNodeId()) return;
    const current = activeTab();
    const nextTab: NodeSettingsTab =
      isNodeTab(current) && isNodeTabAvailable(current) ? current : "general";
    navigate(nodeSettingsPath(nextTab, nextNodeId));
  }

  function tabHref(tabId: SettingsTabId): string {
    switch (activeScope()) {
      case "hub":
        return hubSettingsPath(isHubTab(tabId) ? (tabId as HubSettingsTab) : "general");
      case "node":
        return nodeSettingsPath(
          isNodeTab(tabId) ? (tabId as NodeSettingsTab) : "general",
          routeNodeId(),
        );
      case "browser":
        return browserSettingsPath(
          isBrowserTab(tabId) ? (tabId as BrowserSettingsTab) : "appearance",
        );
    }
  }

  /** Search query: while set, the scope/tabs are replaced by cross-tab search results. */
  const [query, setQuery] = createSignal("");
  const visibleSearchResults = createMemo(() =>
    searchSettings(query()).filter((entry) => entry.scope !== "hub" || isHub()),
  );

  function searchResultHref(entry: SettingsSearchEntry): string {
    const targetNode =
      activeScope() === "node" && nodeStatus().kind === "ok" && routeNodeId() !== ""
        ? routeNodeId()
        : defaultNodeId();
    return settingsPathForTarget(entry.scope, entry.tab, { isHub: isHub(), nodeId: targetNode });
  }

  function searchResultTarget(entry: SettingsSearchEntry): string {
    const scope = SETTINGS_SCOPE_LABELS[resolveSettingsScope(entry.scope, isHub())];
    return `${scope} · ${SETTINGS_TAB_LABELS[entry.tab]}`;
  }

  const fallbackNodeTab = createMemo<NodeSettingsTab>(() => {
    const current = activeTab();
    return isNodeTab(current) && isNodeTabAvailable(current) ? current : "general";
  });

  const isIndexRedirectRoute = createMemo(() => {
    const pathname = location.pathname ?? "";
    if (
      pathname === "/settings" ||
      pathname === "/settings/hub" ||
      pathname === "/settings/browser"
    ) {
      return true;
    }
    return pathname === "/settings/nodes" || /^\/settings\/nodes\/[^/]+$/.test(pathname);
  });

  createEffect(
    () => m.settings_page_title(),
    (title) => {
      const previous = document.title;
      document.title = title;
      return () => {
        document.title = previous;
      };
    },
  );

  return (
    <Show when={!isIndexRedirectRoute()} fallback={<>{props.children}</>}>
      <div class="desk-page">
        <div class="flex flex-col gap-1">
          <h1 class="text-xl font-semibold tracking-tight">{m.settings_title()}</h1>
          <p class="text-muted-foreground text-[13px]">{m.settings_subtitle()}</p>
        </div>

        <div class="relative w-full max-w-md">
          <IconSearch class="text-muted-foreground pointer-events-none absolute top-2.5 left-2.5 size-4" />
          <Input
            placeholder={m.settings_search_placeholder()}
            aria-label={m.settings_search_placeholder()}
            value={query()}
            onValueChange={setQuery}
            class="pl-9"
          />
        </div>

        <Show
          when={query().trim() !== ""}
          fallback={
            <>
              {/* Scope switch: Hub (server) / Node (machine) / Browser (this device) */}
              <div class="grid gap-2 sm:grid-cols-3">
                <For each={scopeOptions()}>
                  {(option) => {
                    const isSelected = () => activeScope() === option.id;
                    return (
                      <Show
                        when={!option.disabled}
                        fallback={
                          <button
                            type="button"
                            disabled
                            aria-current={isSelected() ? "page" : undefined}
                            class="border-border flex cursor-not-allowed items-start gap-2 rounded-lg border px-3 py-2.5 text-left opacity-50"
                          >
                            <option.icon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
                            <span class="flex min-w-0 flex-col">
                              <span class="text-[13px] font-semibold">{option.label}</span>
                              <span class="text-muted-foreground text-[11px] leading-snug">
                                {option.description}
                              </span>
                            </span>
                          </button>
                        }
                      >
                        <a
                          href={option.href}
                          aria-current={isSelected() ? "page" : undefined}
                          class={cn(
                            "flex items-start gap-2 rounded-lg border px-3 py-2.5 text-left transition-colors",
                            isSelected()
                              ? "border-signal bg-signal-soft/60"
                              : "border-border hover:border-line-strong hover:bg-muted/50",
                          )}
                        >
                          <option.icon class="text-muted-foreground mt-0.5 size-4 shrink-0" />
                          <span class="flex min-w-0 flex-col">
                            <span class="text-[13px] font-semibold">{option.label}</span>
                            <span class="text-muted-foreground text-[11px] leading-snug">
                              {option.description}
                            </span>
                          </span>
                        </a>
                      </Show>
                    );
                  }}
                </For>
              </div>

              <Show when={activeScope() === "node" && isHub() && onlineNodes().length > 0}>
                <div class="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/30 px-3 py-2">
                  <IconCpu class="text-muted-foreground size-4" />
                  <Label for="settings-target-node">{m.settings_target_node()}</Label>
                  <Select.Root
                    type="single"
                    value={selectedOnlineNodeId()}
                    onValueChange={(value) => onSelectNode(String(value ?? ""))}
                  >
                    <Select.Trigger
                      id="settings-target-node"
                      size="sm"
                      class="min-w-52 text-[13px]"
                      aria-label={m.settings_target_node()}
                    >
                      <span class="truncate">{selectedNodeLabel()}</span>
                    </Select.Trigger>
                    <Select.Content>
                      <For each={onlineNodes()}>
                        {(node) => (
                          <Select.Item
                            value={node.node_id}
                            label={`${node.name} (${node.node_id})`}
                            class="text-[13px]"
                          >
                            {node.name} ({node.node_id})
                          </Select.Item>
                        )}
                      </For>
                    </Select.Content>
                  </Select.Root>
                  <span class="text-muted-foreground text-[12px]">
                    {m.settings_node_scope_hint()}
                  </span>
                </div>
              </Show>

              <Switch
                fallback={
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
                      <For each={tabsByScope()[activeScope()]}>
                        {(tab) => {
                          const isCurrentTab = () => activeTab() === tab.id;
                          return (
                            <a
                              href={tabHref(tab.id)}
                              data-slot="tabs-trigger"
                              data-active={isCurrentTab() ? "" : undefined}
                              aria-current={isCurrentTab() ? "page" : undefined}
                              class={settingsTabTriggerClass}
                            >
                              {tab.label}
                            </a>
                          );
                        }}
                      </For>
                    </div>

                    <div class="mt-2 flex flex-col gap-3">{props.children}</div>
                  </nav>
                }
              >
                <Match when={activeScope() === "hub" && !isHub()}>
                  <Card class="gap-3 p-5">
                    <div class="flex items-start gap-2">
                      <IconServer class="text-muted-foreground mt-0.5 size-4 shrink-0" />
                      <div class="flex flex-col gap-2">
                        <h2 class="font-medium">{m.settings_scope_hub()}</h2>
                        <p class="text-muted-foreground text-[13px]">
                          {m.settings_scope_hub_unavailable()}
                        </p>
                        <Show when={defaultNodeId() !== null}>
                          <div>
                            <Button
                              href={nodeSettingsPath("general", defaultNodeId())}
                              variant="outline"
                              size="sm"
                            >
                              {m.settings_open_node_settings()}
                            </Button>
                          </div>
                        </Show>
                      </div>
                    </div>
                  </Card>
                </Match>

                <Match when={activeScope() === "node" && nodeStatus().kind === "no_online_nodes"}>
                  <Card class="gap-3 p-5">
                    <div class="flex items-start gap-2">
                      <IconCpu class="text-muted-foreground mt-0.5 size-4 shrink-0" />
                      <div class="flex flex-col gap-1">
                        <h2 class="font-medium">{m.settings_node_empty_title()}</h2>
                        <p class="text-muted-foreground text-[13px]">
                          {m.settings_node_empty_description()}
                        </p>
                      </div>
                    </div>
                  </Card>
                </Match>

                <Match
                  when={
                    activeScope() === "node" &&
                    (nodeStatus().kind === "offline" || nodeStatus().kind === "not_found")
                  }
                >
                  <Card class="gap-3 p-5">
                    <div class="flex items-start gap-2">
                      <IconCpu class="text-muted-foreground mt-0.5 size-4 shrink-0" />
                      <div class="flex flex-col gap-2">
                        <h2 class="font-medium">{m.settings_node_unavailable_title()}</h2>
                        <p class="text-muted-foreground text-[13px]">{unavailableMessage()}</p>
                        <Show when={defaultNodeId() !== null}>
                          <div>
                            <Button
                              href={nodeSettingsPath(fallbackNodeTab(), defaultNodeId())}
                              variant="outline"
                              size="sm"
                            >
                              {m.settings_open_default_node()}
                            </Button>
                          </div>
                        </Show>
                      </div>
                    </div>
                  </Card>
                </Match>

                <Match when={activeScope() === "node" && nodeStatus().kind === "index"}>
                  <>{props.children}</>
                </Match>
              </Switch>
            </>
          }
        >
          <section class="flex flex-col gap-2">
            <h2 class="desk-label">{m.settings_search_results({ query: query().trim() })}</h2>
            <Show
              when={visibleSearchResults().length !== 0}
              fallback={
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_search_empty({ query: query().trim() })}
                </p>
              }
            >
              <ul class="divide-y overflow-hidden rounded-lg border">
                <For each={visibleSearchResults()}>
                  {(result) => (
                    <li>
                      <a
                        href={searchResultHref(result)}
                        onClick={() => setQuery("")}
                        class="hover:bg-accent flex w-full items-center gap-3 px-3 py-2 text-left transition-colors"
                      >
                        <span class="text-[13px]">{result.label}</span>
                        <span class="text-muted-foreground ml-auto text-[11px]">
                          {searchResultTarget(result)}
                        </span>
                      </a>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </section>
        </Show>
      </div>
    </Show>
  );
}
