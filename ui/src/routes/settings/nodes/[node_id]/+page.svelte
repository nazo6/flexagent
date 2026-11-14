<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { nodeSettingsPath } from "#lib/settings-routes.ts";

  $effect(() => {
    void goto(nodeSettingsPath("general", page.params.node_id), { replaceState: true });
  });
</script>
