import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { SubagentItem } from "#lib/chat-groups.ts";
import { m } from "#lib/paraglide/messages.js";
import { Badge } from "#lib/components/ui/badge/index.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import IconBot from "~icons/lucide/bot";
import IconChevronDown from "~icons/lucide/chevron-down";
import IconChevronRight from "~icons/lucide/chevron-right";
import IconLoaderCircle from "~icons/lucide/loader-circle";

export interface SubagentGroupProps {
  subagent: SubagentItem;
  onopen?: (childSessionId: string) => void;
  children?: JSX.Element;
}

/**
 * Collapsible block that holds the work of one child (subagent) session.
 *
 * It opens while the child works and folds once it is done, unless the reader
 * toggled it (same idiom as thoughts).
 */
export default function SubagentGroup(props: SubagentGroupProps) {
  const [userToggled, setUserToggled] = createSignal(false);
  const [userIsOpen, setUserIsOpen] = createSignal(false);
  const running = () => props.subagent.status === "running";
  const open = () => (userToggled() ? userIsOpen() : running());

  function toggle(): void {
    setUserToggled(true);
    setUserIsOpen(!open());
  }

  const statusLabel = () => {
    switch (props.subagent.status) {
      case "running":
        return m.chat_subagent_running();
      case "requires_action":
        return m.chat_subagent_requires_action();
      case "completed":
        return m.chat_subagent_completed();
      case "failed":
        return m.chat_subagent_failed();
      case "cancelled":
        return m.chat_subagent_cancelled();
      default:
        return props.subagent.status;
    }
  };
  const title = () =>
    props.subagent.title ??
    props.subagent.description ??
    props.subagent.childSessionId.slice(0, 8);

  return (
    <div
      class="border-border bg-card/40 rounded-lg border border-dashed"
      style={{ "margin-left": `${Math.max(0, props.subagent.depth - 1) * 0.75}rem` }}
    >
      <div class="flex items-center gap-2 px-3 py-2">
        <button
          type="button"
          class="hover:bg-accent/40 flex min-w-0 flex-1 items-center gap-2 rounded px-1 py-0.5 text-left"
          aria-expanded={open() ? "true" : "false"}
          onClick={toggle}
        >
          {open() ? (
            <IconChevronDown class="text-muted-foreground size-3.5 shrink-0" />
          ) : (
            <IconChevronRight class="text-muted-foreground size-3.5 shrink-0" />
          )}
          <IconBot class="text-signal size-3.5 shrink-0" />
          <span class="min-w-0 truncate text-[13px] font-medium">{title()}</span>
          <Show when={running()}>
            <IconLoaderCircle class="text-signal size-3.5 shrink-0 animate-spin" />
          </Show>
          <Badge variant={running() ? "secondary" : "outline"}>{statusLabel()}</Badge>
        </button>
        <Show when={props.onopen}>
          {(onopen) => (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onopen()(props.subagent.childSessionId)}
              aria-label={m.chat_subagent_open()}
            >
              {m.chat_subagent_open()}
            </Button>
          )}
        </Show>
      </div>
      <Show when={open()}>
        <div class="border-border border-t px-3 py-3">{props.children}</div>
      </Show>
    </div>
  );
}
