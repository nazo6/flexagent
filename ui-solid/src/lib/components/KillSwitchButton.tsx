import { createSignal, Show } from "solid-js";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";
import { cn } from "#lib/utils.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import IconOctagonX from "~icons/lucide/octagon-x";

export interface KillSwitchButtonProps {
  class?: string;
}

/**
 * Emergency stop for every running agent.
 *
 * The trigger is a regular button (not `AlertDialog.Trigger`) because the ported
 * alert dialog keeps the trigger outside Terracotta's root.
 */
export default function KillSwitchButton(props: KillSwitchButtonProps) {
  const [open, setOpen] = createSignal(false);
  const [busy, setBusy] = createSignal(false);

  async function confirmKill(): Promise<void> {
    if (busy()) return;
    setBusy(true);
    try {
      const nodes = await sync.killSwitch("web ui kill switch");
      toast.success(m.shell_kill_switch_sent({ count: nodes }));
      setOpen(false);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Button
        variant="destructive"
        size="sm"
        class={cn("shrink-0 gap-1.5 text-[13px]", props.class)}
        title={m.shell_kill_switch_tooltip()}
        aria-label={m.shell_kill_switch()}
        onClick={() => setOpen(true)}
      >
        <IconOctagonX class="size-3.5 shrink-0" />
        <span>{m.shell_kill_switch()}</span>
      </Button>
      <AlertDialog.Root open={open()} onOpenChange={setOpen}>
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title>{m.shell_kill_switch_confirm_title()}</AlertDialog.Title>
            <AlertDialog.Description>
              {m.shell_kill_switch_confirm_description()}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action variant="destructive" onClick={confirmKill} disabled={busy()}>
              <Show when={busy()} fallback={m.shell_kill_switch_confirm()}>
                {m.shell_kill_switch_sending()}
              </Show>
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
