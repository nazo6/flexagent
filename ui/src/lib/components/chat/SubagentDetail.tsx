import type { SubagentItem } from "#lib/chat-groups.ts";
import type { SessionStatus } from "#lib/generated/bindings.ts";
import type { TimelineItem } from "#lib/sync/reducer.ts";
import { m } from "#lib/paraglide/messages.js";
import { Button } from "#lib/components/ui/button/index.ts";
import ChatTimeline from "./ChatTimeline.tsx";
import IconBot from "~icons/lucide/bot";
import IconX from "~icons/lucide/x";

export interface SubagentDetailProps {
  sessionId: string;
  subagent: SubagentItem;
  items: TimelineItem[];
  status?: SessionStatus | null;
  onclose: () => void;
  /** Called when the user stops a command still running in the subagent. */
  onstopterminal?: (terminalId: string) => Promise<void>;
}

/** Slide-over panel that shows the conversation of one child session. */
export default function SubagentDetail(props: SubagentDetailProps) {
  const title = () =>
    props.subagent.title ?? props.subagent.description ?? props.subagent.childSessionId.slice(0, 8);

  return (
    <>
      <button
        type="button"
        class="fixed inset-0 z-40 cursor-default bg-black/30"
        aria-label={m.chat_subagent_close()}
        onClick={() => props.onclose()}
      />
      <dialog
        open
        class="bg-background border-border fixed inset-y-0 right-0 left-auto z-50 m-0 flex h-full w-full max-w-2xl flex-col border-l p-0 shadow-xl"
        aria-label={m.chat_subagent_title()}
      >
        <header class="border-border flex items-center gap-2 border-b px-4 py-3">
          <IconBot class="text-signal size-4" />
          <div class="min-w-0 flex-1">
            <h2 class="truncate text-sm font-semibold">{title()}</h2>
            <p class="text-muted-foreground truncate text-[11px]">
              {props.subagent.childSessionId}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => props.onclose()}
            aria-label={m.chat_subagent_close()}
          >
            <IconX class="size-4" />
          </Button>
        </header>
        <div class="app-scroll min-h-0 flex-1 overflow-y-auto px-4 py-4">
          <ChatTimeline
            sessionId={props.sessionId}
            items={props.items}
            status={props.status ?? null}
            nested
            subagentTitle={title()}
            onstopterminal={props.onstopterminal}
          />
        </div>
      </dialog>
    </>
  );
}
