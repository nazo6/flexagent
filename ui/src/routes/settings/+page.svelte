<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import AgentManager from "#lib/components/settings/AgentManager.svelte";
  import AuditLog from "#lib/components/settings/AuditLog.svelte";
  import DiagnosticsPanel from "#lib/components/settings/DiagnosticsPanel.svelte";
  import SettingsCatalog from "#lib/components/settings/SettingsCatalog.svelte";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
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
  import ShieldIcon from "@lucide/svelte/icons/shield";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";

  const isHub = $derived(sync.connection.role === "hub");

  /** Selected settings tab (reflects deep links such as `?tab=agents`). */
  let activeTab = $state(page.url.searchParams.get("tab") ?? "bootstrap");

  /**
   * Switch tabs and reflect the choice in the URL (reloads / copied links land on the tab).
   *
   * Only a user tab change writes the URL; `replace` keeps tabs out of the back/forward
   * stack, and `shallow` avoids a router navigation for a query-only change.
   */
  function selectTab(value: string): void {
    activeTab = value;
    const url = new URL(location.href);
    if (url.searchParams.get("tab") === value) return;
    url.searchParams.set("tab", value);
    void goto(url, { shallow: true, replace: true });
  }

  /** Theme preference stored in this browser (`system` / `light` / `dark`). */
  const themePreference = $derived(userPrefersMode.current);

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
  /** Catalog search query (filters bootstrap and settings rows). */
  let query = $state("");

  // --- Node pairing (hub only) ---
  const tokens = createResource<NodeTokenSummary[]>(() => sync.connection.client.nodeTokens(), {
    onError: (error) => toast.error(error instanceof Error ? error.message : String(error)),
  });

  // Issue a new node token
  let issueOpen = $state(false);
  let issueNodeId = $state("");
  let issueBusy = $state(false);
  /** Plaintext token right after issuance (cannot be shown again once this dialog is closed). */
  let issued = $state<IssueNodeTokenResponse | null>(null);

  // Token revocation confirmation
  let revokeTarget = $state<string | null>(null);
  let revokeBusy = $state(false);

  // --- Client auth token ---
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
      toast.error(error instanceof Error ? error.message : String(error));
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
      toast.error(error instanceof Error ? error.message : String(error));
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
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      rotateBusy = false;
    }
  }
</script>

<div class="desk-page">
  <div class="flex flex-col gap-1">
    <h1 class="text-xl font-semibold tracking-tight">{m.settings_title()}</h1>
    <p class="text-muted-foreground text-[13px]">
      {m.settings_subtitle()}
    </p>
  </div>

  <Input placeholder="Search settings…" bind:value={query} class="max-w-md" />

  <Tabs.Root value={activeTab} onValueChange={selectTab}>
    <Tabs.List variant="line" class="w-full overflow-x-auto border-b">
      <Tabs.Trigger value="bootstrap">Bootstrap</Tabs.Trigger>
      <Tabs.Trigger value="machine">This machine</Tabs.Trigger>
      <Tabs.Trigger value="projects">Projects</Tabs.Trigger>
      <Tabs.Trigger value="hub">Hub</Tabs.Trigger>
      <Tabs.Trigger value="agents">{m.settings_tab_agents()}</Tabs.Trigger>
      <Tabs.Trigger value="browser">Browser</Tabs.Trigger>
      <Tabs.Trigger value="audit">{m.settings_tab_audit()}</Tabs.Trigger>
    </Tabs.List>

    <Tabs.Content value="bootstrap" class="mt-2 flex flex-col gap-3">
      <SettingsCatalog section="bootstrap" {query} />
      <DiagnosticsPanel />
    </Tabs.Content>

    <Tabs.Content value="machine" class="mt-2 flex flex-col gap-3">
      <SettingsCatalog section="machine" {query} />
      <Card class="gap-3 p-4">
        <div class="flex items-start gap-2">
          <ShieldIcon class="text-muted-foreground mt-0.5 size-4" />
          <div class="flex flex-1 flex-col gap-1">
            <h2 class="font-medium">{m.settings_client_token_title()}</h2>
            <p class="text-muted-foreground text-[13px]">
              {m.settings_client_token_description()}
              <code>~/.flexagent/auth_token</code>
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

    <Tabs.Content value="projects" class="mt-2 flex flex-col gap-3">
      <SettingsCatalog section="projects" {query} />
    </Tabs.Content>

    <Tabs.Content value="hub" class="mt-2 flex flex-col gap-3">
      <SettingsCatalog section="hub" {query} />
      <Card class="gap-3 p-4">
        <div class="flex items-start gap-2">
          <KeyRoundIcon class="text-muted-foreground mt-0.5 size-4" />
          <div class="flex flex-1 flex-col gap-2">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 class="font-medium">{m.settings_node_pairing_title()}</h2>
                <p class="text-muted-foreground text-[13px]">
                  {m.settings_node_pairing_description()}
                  (<code>fxg node start</code>, <code>~/.flexagent/node_token</code>)
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
                    <Badge variant="outline" class="text-[11px]">{m.settings_badge_issued()}</Badge>
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

    <Tabs.Content value="agents" class="mt-2 flex flex-col gap-3">
      <AgentManager />
    </Tabs.Content>

    <Tabs.Content value="browser" class="mt-2 flex flex-col gap-3">
      <Card class="gap-4 p-5">
        <div class="flex flex-col gap-1">
          <h2 class="text-sm font-semibold">{m.settings_appearance_theme()}</h2>
          <p class="text-muted-foreground text-[13px]">{m.settings_appearance_theme_note()}</p>
        </div>
        <div class="grid max-w-sm gap-1.5">
          <select
            id="browser-theme"
            aria-label={m.settings_appearance_theme()}
            class="border-input bg-background h-8 rounded-md border px-2 text-[13px]"
            value={themePreference}
            onchange={(event) => setMode(event.currentTarget.value as "system" | "light" | "dark")}
          >
            <option value="system">{m.settings_browser_theme_system()}</option>
            <option value="light">{m.settings_browser_theme_light()}</option>
            <option value="dark">{m.settings_browser_theme_dark()}</option>
          </select>
        </div>
      </Card>

      <Card class="gap-4 p-5">
        <div class="flex flex-col gap-1">
          <h2 class="text-sm font-semibold">{m.settings_appearance_palette()}</h2>
          <p class="text-muted-foreground text-[13px]">{m.settings_appearance_palette_note()}</p>
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
        <p class="text-muted-foreground text-[13px]">{m.settings_appearance_storage_note()}</p>
      </Card>

      <Card class="gap-4 p-5">
        <div class="flex flex-col gap-1">
          <h2 class="text-sm font-semibold">{m.settings_browser_send_shortcut()}</h2>
          <p class="text-muted-foreground text-[13px]">{m.settings_browser_send_shortcut_note()}</p>
        </div>
        <div class="grid max-w-sm gap-1.5">
          <select
            id="browser-send-shortcut"
            aria-label={m.settings_browser_send_shortcut()}
            class="border-input bg-background h-8 rounded-md border px-2 text-[13px]"
            value={prefs.sendShortcut}
            onchange={(event) =>
              prefs.setSendShortcut(event.currentTarget.value as "enter" | "modifier_enter")}
          >
            <option value="enter">{m.settings_browser_send_shortcut_enter()}</option>
            <option value="modifier_enter">{m.settings_browser_send_shortcut_modifier()}</option>
          </select>
        </div>
      </Card>

      <Card class="gap-3 p-4">
        <h2 class="text-sm font-semibold">{m.settings_browser_pinned()}</h2>
        <p class="text-muted-foreground text-[13px]">{m.settings_browser_storage_note()}</p>
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

    <Tabs.Content value="audit" class="mt-2 flex flex-col gap-3">
      <AuditLog />
    </Tabs.Content>
  </Tabs.Root>
</div>

<!-- Issue node token -->
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
  <Dialog.Content class="sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>{m.settings_node_token_issued_title()}</Dialog.Title>
      <Dialog.Description>
        {m.settings_node_token_issued_description()}
        <code>~/.flexagent/node_token</code>
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
            class="bg-muted min-w-0 flex-1 truncate rounded-md border px-2 py-1 font-mono text-[13px]"
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
      <pre
        class="bg-muted overflow-x-auto rounded-md border p-2 text-[11px] leading-relaxed">{`${m.settings_node_token_save_comment()}\necho "${issued?.token ?? ""}" > ~/.flexagent/node_token`}</pre>
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

<!-- Revoke node token confirmation -->
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
