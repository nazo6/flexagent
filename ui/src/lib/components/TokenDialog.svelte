<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Input } from "#lib/components/ui/input/index.ts";
  import { Label } from "#lib/components/ui/label/index.ts";
  import { connection } from "#lib/stores/app.svelte.ts";
  import { loginAndStart } from "#lib/stores/app.svelte.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";
  import KeyRoundIcon from "@lucide/svelte/icons/key-round";

  let token = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    error = null;
    try {
      await loginAndStart(token);
      token = "";
      toast.success(m.token_connected_toast());
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog.Root open={connection.tokenRequired}>
  <Dialog.Content
    class="sm:max-w-md"
    showCloseButton={false}
    onInteractOutside={(event) => event.preventDefault()}
    onEscapeKeydown={(event) => event.preventDefault()}
  >
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <KeyRoundIcon class="size-4" />
        {m.token_dialog_title()}
      </Dialog.Title>
      <Dialog.Description>
        {#if connection.role === "hub"}
          {m.token_dialog_description_hub()}
        {:else}
          {m.token_dialog_description_local_node()}
        {/if}
        {m.token_cookie_note_before()} <code>fxg_session</code>
        {m.token_cookie_note_after()}
      </Dialog.Description>
    </Dialog.Header>

    <form class="flex flex-col gap-4" onsubmit={submit}>
      <div class="grid gap-2">
        <Label for="fxg-token">{m.token_label()}</Label>
        <Input
          id="fxg-token"
          type="password"
          autocomplete="off"
          placeholder={connection.role === "hub"
            ? m.token_placeholder_hub()
            : m.token_placeholder_node()}
          bind:value={token}
          disabled={busy}
        />
        <p class="text-muted-foreground text-[13px]">
          {#if connection.role === "hub"}
            {m.token_help_hub_prefix()} <code>auth_token</code>
            {m.token_help_hub_mid()}
            <code>~/.flexagent/auth_token</code>{m.token_help_hub_docker()}
            <code>docker compose exec server cat /data/auth_token</code>.
          {:else}
            {m.token_help_node_prefix()} <code>~/.flexagent/auth_token</code>
            {m.token_help_node_mid()} <code>fxg node access-token show</code>
            {m.token_help_node_docker()}
            <code>docker compose exec node fxg node access-token show</code>).
          {/if}
        </p>
      </div>

      {#if error}
        <p class="text-destructive text-sm">{error}</p>
      {/if}

      <Button type="submit" disabled={busy || token.trim() === ""}>
        {busy ? m.token_verifying() : m.token_connect()}
      </Button>
    </form>
  </Dialog.Content>
</Dialog.Root>
