import { m } from "#lib/paraglide/messages.js";
import IconGitFork from "~icons/lucide/git-fork";

export interface ForkOriginProps {
  parentSessionId: string;
  title?: string | null;
}

/** Marker between the history copied from a parent session and live work. */
export default function ForkOrigin(props: ForkOriginProps) {
  const label = () => props.title ?? props.parentSessionId.slice(0, 8);

  return (
    <div class="text-muted-foreground flex items-center gap-2 py-1 text-[11px]">
      <span class="bg-border/60 h-px flex-1" />
      <a
        href={`/sessions/${props.parentSessionId}`}
        class="border-border bg-card hover:bg-accent hover:text-foreground inline-flex max-w-[80%] items-center gap-1.5 rounded-full border px-2.5 py-1 shadow-sm transition-colors"
        title={m.session_fork_origin({ title: label() })}
      >
        <IconGitFork class="size-3 shrink-0" />
        <span class="truncate">{m.session_fork_origin({ title: label() })}</span>
      </a>
      <span class="bg-border/60 h-px flex-1" />
    </div>
  );
}
