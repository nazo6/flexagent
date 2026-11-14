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
  import type { IssueNodeTokenResponse, NodeTokenSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { createResource } from "#lib/stores/resource.svelte.ts";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import KeyRoundIcon from "@lucide/svelte/icons/key-round";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
  import { toast } from "svelte-sonner";

  const isHub = $derived(sync.connection.role === "hub");

  const tokens = createResource<NodeTokenSummary[]>(() => sync.connection.client.nodeTokens(), {
    onError: (error) => toast.error(errorMessage(error)),
  });

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

  let revokeTarget = $state<string | null>(null);
  let revokeBusy = $state(false);

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
</script>

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
