<script lang="ts">
  import { errorMessage } from "#lib/api/error-messages.ts";
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import ShieldIcon from "@lucide/svelte/icons/shield";
  import { toast } from "svelte-sonner";

  const isHub = $derived(sync.connection.role === "hub");
  let rotateOpen = $state(false);
  let rotateBusy = $state(false);

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
