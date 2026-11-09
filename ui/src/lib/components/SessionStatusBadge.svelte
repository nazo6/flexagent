<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { sessionStatusLabel, sessionStatusVariant } from "#lib/session-status.ts";
  import type { SessionStatus } from "#lib/generated/bindings.ts";

  let { status }: { status: SessionStatus } = $props();
</script>

<Badge
  variant={sessionStatusVariant(status)}
  class={status === "running"
    ? "border-signal/25 bg-signal-soft text-signal"
    : status === "waiting_permission" || status === "waiting_input"
      ? "border-warn/25 bg-warn-soft text-warn"
      : ""}
>
  {sessionStatusLabel(status)}
</Badge>
