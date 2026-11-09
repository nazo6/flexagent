<script lang="ts">
  import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { toast } from "svelte-sonner";
  import OctagonXIcon from "@lucide/svelte/icons/octagon-x";

  interface Props {
    class?: string;
  }

  let { class: className }: Props = $props();

  let busy = $state(false);

  async function confirmKill() {
    if (busy) return;
    busy = true;
    try {
      const nodes = await sync.killSwitch("web ui kill switch");
      toast.success(m.shell_kill_switch_sent({ count: nodes }));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      busy = false;
    }
  }
</script>

<AlertDialog.Root>
  <AlertDialog.Trigger>
    {#snippet child({ props })}
      <Button
        variant="destructive"
        size="sm"
        {...props}
        class={cn("shrink-0 gap-1.5 text-ui", className)}
        title={m.shell_kill_switch_tooltip()}
      >
        <OctagonXIcon class="size-3.5 shrink-0" />
        <span>{m.shell_kill_switch()}</span>
      </Button>
    {/snippet}
  </AlertDialog.Trigger>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>{m.shell_kill_switch_confirm_title()}</AlertDialog.Title>
      <AlertDialog.Description>
        {m.shell_kill_switch_confirm_description()}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={confirmKill} disabled={busy}>
        {busy ? m.shell_kill_switch_sending() : m.shell_kill_switch_confirm()}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
