import { createSignal } from "solid-js";
import IconRefreshCw from "~icons/lucide/refresh-cw";
import IconShield from "~icons/lucide/shield";
import { errorMessage } from "#lib/api/error-messages.ts";
import * as AlertDialog from "#lib/components/ui/alert-dialog/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { Card } from "#lib/components/ui/card/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { sync } from "#lib/stores/app.ts";

/** Web access token card (rotate the token used by browsers / CLI). */
export default function ClientTokenCard() {
  const isHub = () => sync.connection.role === "hub";
  const [rotateOpen, setRotateOpen] = createSignal(false);
  const [rotateBusy, setRotateBusy] = createSignal(false);

  async function confirmRotate(): Promise<void> {
    if (rotateBusy()) return;
    setRotateBusy(true);
    try {
      const result = await sync.connection.client.rotateAuthToken();
      // The rotate response also sets the new `fxg_session` cookie, so no
      // second login round-trip is needed (a network failure here must not
      // leave the browser locked out).
      await sync.connection.applyToken(result.token);
      sync.restart();
      toast.success(m.settings_token_regenerated());
      setRotateOpen(false);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setRotateBusy(false);
    }
  }

  return (
    <>
      <Card class="gap-3 p-4">
        <div class="flex items-start gap-2">
          <IconShield class="text-muted-foreground mt-0.5 size-4" />
          <div class="flex flex-1 flex-col gap-1">
            <h2 class="font-medium">{m.settings_client_token_title()}</h2>
            <p class="text-muted-foreground text-[13px]">
              {m.settings_client_token_description()}{" "}
              <code>&lt;data&gt;/flexagent/{isHub() ? "hub" : "node"}/auth_token</code>{" "}
              {m.settings_client_token_description_ref()}
            </p>
            <div class="mt-1">
              <Button variant="outline" size="sm" onClick={() => setRotateOpen(true)}>
                <IconRefreshCw />
                {m.settings_regenerate_token()}
              </Button>
            </div>
          </div>
        </div>
      </Card>

      <AlertDialog.Root open={rotateOpen()} onOpenChange={setRotateOpen}>
        <AlertDialog.Content>
          <AlertDialog.Header>
            <AlertDialog.Title>{m.settings_rotate_token_title()}</AlertDialog.Title>
            <AlertDialog.Description class="text-sm">
              {m.settings_rotate_token_description()}
            </AlertDialog.Description>
          </AlertDialog.Header>
          <AlertDialog.Footer>
            <AlertDialog.Cancel disabled={rotateBusy()}>{m.common_cancel()}</AlertDialog.Cancel>
            <AlertDialog.Action disabled={rotateBusy()} onClick={() => void confirmRotate()}>
              {rotateBusy() ? m.settings_regenerating() : m.settings_regenerate()}
            </AlertDialog.Action>
          </AlertDialog.Footer>
        </AlertDialog.Content>
      </AlertDialog.Root>
    </>
  );
}
