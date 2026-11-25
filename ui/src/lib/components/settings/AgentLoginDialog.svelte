<script lang="ts">
  /**
   * Agent authentication dialog.
   *
   * Agent-managed (`agent` kind) methods run through the node and complete in
   * this dialog. Terminal methods need an interactive shell, so they are shown
   * as the exact `fxg agents login` command instead.
   */
  import { untrack } from "svelte";
  import { errorMessage } from "#lib/api/error-messages.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import type { AgentAuthMethodInfo, AgentSummary } from "#lib/generated/bindings.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { sync } from "#lib/stores/app.svelte.ts";
  import { toast } from "svelte-sonner";
  import CheckCircle2Icon from "@lucide/svelte/icons/check-circle-2";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import LogInIcon from "@lucide/svelte/icons/log-in";
  import SquareTerminalIcon from "@lucide/svelte/icons/square-terminal";

  let {
    nodeId,
    agent,
    open = $bindable(false),
  }: {
    nodeId: string;
    /** Agent to authenticate; kept while the dialog animates closed. */
    agent: AgentSummary | null;
    open?: boolean;
  } = $props();

  let methods = $state<AgentAuthMethodInfo[] | null>(null);
  let loading = $state(false);
  let busyMethod = $state<string | null>(null);
  let signedIn = $state(false);

  $effect(() => {
    const target = agent;
    if (!open || target === null) return;
    untrack(() => void load(target));
  });

  async function load(target: AgentSummary): Promise<void> {
    loading = true;
    methods = null;
    signedIn = false;
    try {
      const response = await sync.connection.client.agentAuthMethods(nodeId, target.id);
      methods = response.methods;
    } catch (error) {
      methods = [];
      toast.error(errorMessage(error));
    } finally {
      loading = false;
    }
  }

  async function signIn(method: AgentAuthMethodInfo): Promise<void> {
    const target = agent;
    if (target === null || busyMethod !== null) return;
    busyMethod = method.id;
    try {
      await sync.connection.client.agentLogin(nodeId, target.id, method.id);
      signedIn = true;
      toast.success(m.settings_agent_login_success({ name: target.name }));
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      busyMethod = null;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {agent === null
          ? m.settings_agent_login()
          : m.settings_agent_login_title({ name: agent.name })}
      </Dialog.Title>
      <Dialog.Description>
        {m.settings_agent_login_description()}
      </Dialog.Description>
    </Dialog.Header>

    {#if agent !== null}
      {#if loading}
        <div class="text-muted-foreground flex items-center gap-2 text-sm">
          <LoaderCircleIcon class="size-4 animate-spin" />
          {m.settings_agent_login_loading()}
        </div>
      {:else if signedIn}
        <div class="flex items-center gap-2 text-sm">
          <CheckCircle2Icon class="size-4 text-ok" />
          {m.settings_agent_login_success({ name: agent.name })}
        </div>
      {:else if methods !== null}
        {#if methods.length === 0}
          <p class="text-muted-foreground text-sm">{m.settings_agent_login_none()}</p>
        {:else}
          <div class="flex flex-col gap-2">
            {#each methods as method (method.id)}
              <div class="rounded-md border border-border p-3">
                <div class="flex items-start gap-2">
                  <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span class="text-sm font-medium">{method.name}</span>
                    {#if method.description}
                      <span class="text-muted-foreground text-[12px]">{method.description}</span>
                    {/if}
                  </div>
                  {#if method.kind === "agent"}
                    <Button
                      size="sm"
                      disabled={busyMethod !== null}
                      onclick={() => void signIn(method)}
                    >
                      {#if busyMethod === method.id}
                        <LoaderCircleIcon class="animate-spin" />
                      {:else}
                        <LogInIcon />
                      {/if}
                      {m.settings_agent_login()}
                    </Button>
                  {/if}
                </div>

                {#if method.kind === "terminal"}
                  <div class="mt-2 flex flex-col gap-1">
                    <span class="text-muted-foreground flex items-center gap-1 text-[12px]">
                      <SquareTerminalIcon class="size-3" />
                      {m.settings_agent_login_terminal_hint()}
                    </span>
                    <code
                      class="bg-muted/50 overflow-x-auto rounded px-2 py-1 font-mono text-[12px]"
                    >
                      fxg agents login {agent.id} --method {method.id}
                    </code>
                  </div>
                {:else if method.kind !== "agent"}
                  <p class="text-muted-foreground mt-2 text-[12px]">
                    {m.settings_agent_login_unknown()}
                  </p>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}

    <Dialog.Footer>
      <Dialog.Close>{m.common_close()}</Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
