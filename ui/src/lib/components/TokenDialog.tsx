import { createSignal, Show } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import { Input } from "#lib/components/ui/input/index.ts";
import { Label } from "#lib/components/ui/label/index.ts";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { m } from "#lib/paraglide/messages.js";
import { connection, loginAndStart } from "#lib/stores/app.ts";
import IconKeyRound from "~icons/lucide/key-round";
import { errorMessage } from "#lib/api/error-messages.ts";

/**
 * Auth token dialog, ported from `ui/src/lib/components/TokenDialog.svelte`.
 *
 * Shown while the server requires a token (`connection.tokenRequired`). It is
 * not dismissible: the dialog wrapper is controlled, so outside clicks and
 * Escape ask to close but the shell keeps it open until login succeeds.
 */
export default function TokenDialog() {
  const [token, setToken] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const submit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      await loginAndStart(token());
      setToken("");
      toast.success(m.token_connected_toast());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog.Root open={connection.tokenRequired}>
      <Dialog.Content class="sm:max-w-md" showCloseButton={false}>
        <Dialog.Header>
          <Dialog.Title class="flex items-center gap-2">
            <IconKeyRound class="size-4" />
            {m.token_dialog_title()}
          </Dialog.Title>
          <Dialog.Description>
            <Show
              when={connection.role === "hub"}
              fallback={m.token_dialog_description_local_node()}
            >
              {m.token_dialog_description_hub()}
            </Show>{" "}
            {m.token_cookie_note_before()} <code>fxg_session</code> {m.token_cookie_note_after()}
          </Dialog.Description>
        </Dialog.Header>

        <form class="flex flex-col gap-4" onSubmit={submit}>
          <div class="grid gap-2">
            <Label for="fxg-token">{m.token_label()}</Label>
            <Input
              id="fxg-token"
              type="password"
              autocomplete="off"
              placeholder={
                connection.role === "hub" ? m.token_placeholder_hub() : m.token_placeholder_node()
              }
              value={token()}
              onValueChange={setToken}
              disabled={busy()}
            />
            <p class="text-muted-foreground text-[13px]">
              <Show
                when={connection.role === "hub"}
                fallback={
                  <>
                    {m.token_help_node_prefix()} <code>&lt;data&gt;/flexagent/node/auth_token</code>{" "}
                    {m.token_help_node_mid()} <code>fxg node access-token show</code>{" "}
                    {m.token_help_node_docker()}{" "}
                    <code>docker compose exec node fxg node access-token show</code>).
                  </>
                }
              >
                {m.token_help_hub_prefix()} <code>auth_token</code> {m.token_help_hub_mid()}{" "}
                <code>&lt;data&gt;/flexagent/hub/auth_token</code> {m.token_help_hub_docker()}{" "}
                <code>docker compose exec hub cat /var/lib/flexagent/data/hub/auth_token</code>.
              </Show>
            </p>
          </div>

          <Show when={error() !== null}>
            <p class="text-destructive text-sm">{error()}</p>
          </Show>

          <Button type="submit" disabled={busy() || token().trim() === ""}>
            {busy() ? m.token_verifying() : m.token_connect()}
          </Button>
        </form>
      </Dialog.Content>
    </Dialog.Root>
  );
}
