<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import { sessionStatusLabel, sessionStatusVariant } from '$lib/session-status';
  import type { SessionStatus } from '$lib/generated/SessionStatus';

  let { status }: { status: SessionStatus } = $props();
</script>

<Badge variant={sessionStatusVariant(status)}>{sessionStatusLabel(status)}</Badge>
