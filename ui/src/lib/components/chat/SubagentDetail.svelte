<script lang="ts">
  import type { SubagentItem } from "#lib/chat-groups.ts";
  import type { SessionStatus } from "#lib/generated/bindings.ts";
  import type { TimelineItem } from "#lib/sync/reducer.ts";
  import { m } from "#lib/paraglide/messages.js";
  import { Button } from "#lib/components/ui/button/index.ts";
  import BotIcon from "@lucide/svelte/icons/bot";
  import XIcon from "@lucide/svelte/icons/x";
  import ChatTimeline from "./ChatTimeline.svelte";

  let {
    sessionId,
    subagent,
    items,
    status = null,
    onclose,
  }: {
    sessionId: string;
    subagent: SubagentItem;
    items: TimelineItem[];
    status?: SessionStatus | null;
    onclose: () => void;
  } = $props();

  const title = $derived(
    subagent.title ?? subagent.description ?? subagent.childSessionId.slice(0, 8),
  );
</script>

<button
  type="button"
  class="fixed inset-0 z-40 cursor-default bg-black/30"
  aria-label={m.chat_subagent_close()}
  onclick={onclose}
></button>
<dialog
  open
  class="bg-background border-border fixed inset-y-0 right-0 left-auto z-50 m-0 flex h-full w-full max-w-2xl flex-col border-l p-0 shadow-xl"
  aria-label={m.chat_subagent_title()}
>
  <header class="border-border flex items-center gap-2 border-b px-4 py-3">
    <BotIcon class="text-signal size-4" />
    <div class="min-w-0 flex-1">
      <h2 class="truncate text-sm font-semibold">{title}</h2>
      <p class="text-muted-foreground truncate text-[11px]">{subagent.childSessionId}</p>
    </div>
    <Button variant="ghost" size="icon" onclick={onclose} aria-label={m.chat_subagent_close()}>
      <XIcon class="size-4" />
    </Button>
  </header>
  <div class="app-scroll min-h-0 flex-1 overflow-y-auto px-4 py-4">
    <ChatTimeline {sessionId} {items} {status} nested subagentTitle={title} />
  </div>
</dialog>
