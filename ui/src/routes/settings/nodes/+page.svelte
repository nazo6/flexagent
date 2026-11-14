<script lang="ts">
  import { goto } from "$app/navigation";
  import { nodeSettingsPath, resolveDefaultNodeId } from "#lib/settings-routes.ts";
  import { sync } from "#lib/stores/app.svelte.ts";

  const isHub = $derived(sync.connection.role === "hub");
  const defaultNodeId = $derived(
    resolveDefaultNodeId(isHub, sync.nodes, sync.connection.systemInfo?.node_id),
  );

  $effect(() => {
    if (defaultNodeId !== null) {
      void goto(nodeSettingsPath("general", defaultNodeId), { replaceState: true });
    }
  });
</script>
