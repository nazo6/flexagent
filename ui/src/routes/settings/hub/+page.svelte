<script lang="ts">
  import { goto } from "$app/navigation";
  import { hubSettingsPath } from "#lib/settings-routes.ts";

  $effect(() => {
    void goto(hubSettingsPath("general"), { replaceState: true });
  });
</script>
