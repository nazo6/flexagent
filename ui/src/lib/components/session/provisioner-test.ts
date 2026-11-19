import { createSignal } from "solid-js";
import type { ProvisionerTestResponse } from "#lib/generated/bindings.ts";
import { errorMessage } from "#lib/api/error-messages.ts";
import { m } from "#lib/paraglide/messages.js";
import { toast } from "#lib/components/ui/sonner/index.ts";
import { sync } from "#lib/stores/app.ts";

/**
 * Connectivity test of one ephemeral-VM provisioner (hub only).
 *
 * The dialog opens before the request so the provisioning log is visible while
 * the VM boots; the outcome is reported both in the dialog and as a toast.
 */
export function createProvisionerTest(onStart: () => void) {
  const [busy, setBusy] = createSignal(false);
  const [result, setResult] = createSignal<ProvisionerTestResponse | null>(null);
  const [dialogOpen, setDialogOpen] = createSignal(false);

  async function run(name: string): Promise<void> {
    if (busy() || !name) return;
    onStart();
    setBusy(true);
    setResult(null);
    setDialogOpen(true);
    try {
      const outcome = await sync.connection.client.testProvisioner(name);
      setResult(outcome);
      if (outcome.ok) {
        toast.success(m.new_session_provisioner_test_success({ name }));
      } else {
        toast.error(
          m.new_session_provisioner_test_failed({
            name,
            error: outcome.error ?? m.new_session_unknown_error(),
          }),
        );
      }
    } catch (error) {
      toast.error(errorMessage(error));
      setResult({
        name,
        ok: false,
        node_id: null,
        log_lines: [],
        error: errorMessage(error),
      });
    } finally {
      setBusy(false);
    }
  }

  return { busy, result, dialogOpen, setDialogOpen, run };
}
