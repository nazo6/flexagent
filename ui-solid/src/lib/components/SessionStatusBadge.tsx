import { Badge } from "#lib/components/ui/badge/index.ts";
import type { SessionStatus } from "#lib/generated/bindings.ts";
import { sessionStatusLabel, sessionStatusVariant } from "#lib/session-status.ts";

/** Status badge for a session row. */
export default function SessionStatusBadge(props: { status: SessionStatus }) {
  return (
    <Badge
      variant={sessionStatusVariant(props.status)}
      class={
        props.status === "running"
          ? "border-signal/25 bg-signal-soft text-signal"
          : props.status === "waiting_permission" || props.status === "waiting_input"
            ? "border-warn/25 bg-warn-soft text-warn"
            : ""
      }
    >
      {sessionStatusLabel(props.status)}
    </Badge>
  );
}
