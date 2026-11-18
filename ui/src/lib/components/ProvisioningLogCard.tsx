import { Badge } from "#lib/components/ui/badge/index.ts";
import { m } from "#lib/paraglide/messages.js";
import IconWrench from "~icons/lucide/wrench";
import { Show, createEffect, createSignal } from "solid-js";

/** Collapsible provisioning log shown while a node is being prepared. */
export default function ProvisioningLogCard(props: { lines: string[] }) {
  const [open, setOpen] = createSignal(true);
  let scrollEl: HTMLPreElement | undefined;

  createEffect(
    () => ({ count: props.lines.length, open: open(), el: scrollEl }),
    ({ open, el }) => {
      if (el !== undefined && open && props.lines.length > 0) {
        el.scrollTop = el.scrollHeight;
      }
    },
  );

  return (
    <Show when={props.lines.length > 0}>
      <div class="bg-card/60 rounded-lg border">
        <button
          type="button"
          class="flex w-full items-center gap-2 px-3 py-2 text-left text-[13px] font-medium"
          onClick={() => setOpen((value) => !value)}
        >
          <IconWrench class="size-3.5" />
          Environment Provisioning Log
          <Badge variant="secondary">
            {props.lines.length === 1
              ? m.chat_provisioning_line({ count: props.lines.length })
              : m.chat_provisioning_lines({ count: props.lines.length })}
          </Badge>
          <span class="text-muted-foreground ml-auto">
            {open() ? m.chat_collapse() : m.chat_expand()}
          </span>
        </button>
        <Show when={open()}>
          <pre
            ref={(el) => (scrollEl = el)}
            class="bg-muted/30 max-h-48 overflow-auto border-t p-2 font-mono text-[13px] whitespace-pre-wrap"
          >
            {props.lines.join("\n")}
          </pre>
        </Show>
      </div>
    </Show>
  );
}
