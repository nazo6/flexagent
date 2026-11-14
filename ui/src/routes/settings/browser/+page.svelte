<script lang="ts">
  import { goto } from "$app/navigation";
  import { browserSettingsPath } from "#lib/settings-routes.ts";

  $effect(() => {
    void goto(browserSettingsPath("appearance"), { replaceState: true });
  });
</script>
