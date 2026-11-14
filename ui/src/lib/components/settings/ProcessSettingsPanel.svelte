<script lang="ts">
  import KillSwitchButton from "#lib/components/KillSwitchButton.svelte";
  import DiagnosticsPanel from "#lib/components/settings/DiagnosticsPanel.svelte";
  import SettingsCatalog from "#lib/components/settings/SettingsCatalog.svelte";
  import { Card } from "#lib/components/ui/card/index.ts";
  import { m } from "#lib/paraglide/messages.js";
  import OctagonXIcon from "@lucide/svelte/icons/octagon-x";
</script>

<SettingsCatalog section="bootstrap" />
<DiagnosticsPanel />
<Card class="border-destructive/40 gap-3 p-4">
  <div class="flex flex-wrap items-start justify-between gap-3">
    <div class="flex items-start gap-2">
      <OctagonXIcon class="text-destructive mt-0.5 size-4 shrink-0" />
      <div class="flex flex-col gap-1">
        <h2 class="font-medium">{m.settings_emergency_stop_title()}</h2>
        <p class="text-muted-foreground text-[13px]">
          {m.settings_emergency_stop_description()}
        </p>
      </div>
    </div>
    <KillSwitchButton />
  </div>
</Card>
