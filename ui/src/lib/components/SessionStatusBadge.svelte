<script lang="ts">
  import { Badge } from "#lib/components/ui/badge/index.ts";
  import { sessionStatusLabel, sessionStatusVariant } from "#lib/session-status.ts";
  import type { SessionStatus } from "#lib/generated/bindings.ts";

  let { status }: { status: SessionStatus } = $props();
</script>

<Badge variant={sessionStatusVariant(status)}>{sessionStatusLabel(status)}</Badge>
