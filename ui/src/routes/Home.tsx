import { Portal } from "@solidjs/web";
import type { NodeSummary } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { For, createSignal } from "solid-js";
import {
  Dialog,
  DialogDescription,
  DialogOverlay,
  DialogPanel,
  DialogTitle,
} from "terracotta/dialog";
import { Toast, Toaster, ToasterStore, useToaster } from "terracotta/toast";
import IconCircleCheck from "~icons/lucide/circle-check";
import IconPlus from "~icons/lucide/plus";

/**
 * Phase 0 smoke screen.
 *
 * Proves the stack end to end: Solid 2, `@solidjs/router`, Tailwind with the
 * Desk tokens, Terracotta (dialog + toast), `unplugin-icons`, Paraglide and the
 * shared specta bindings from `ui/`.
 */

const notices = new ToasterStore<string>();

function notify(message: string) {
  const id = notices.create(message);
  setTimeout(() => notices.remove(id), 4000);
}

export default function Home() {
  const [open, setOpen] = createSignal(false);
  // Type-only proof that the Svelte app's generated bindings resolve.
  const [nodes] = createSignal<NodeSummary[]>([]);
  const queue = useToaster(notices);

  return (
    <div class="desk-page">
      <header class="flex flex-wrap items-end justify-between gap-4">
        <div class="flex flex-col gap-1">
          <h1 class="text-2xl font-semibold tracking-tight">Nodes</h1>
          <p class="text-muted-ink text-sm">
            Phase 0 smoke: Solid 2, router, Desk tokens, Terracotta, icons, Paraglide and shared
            bindings.
          </p>
        </div>
        <div class="flex items-center gap-2">
          <button
            type="button"
            class="border-line bg-surface hover:bg-chip-hover h-8 rounded-md border px-3 text-xs font-medium"
            onClick={() => notify("This toast comes from a store outside the component tree.")}
          >
            Show toast
          </button>
          <button
            type="button"
            class="bg-primary text-primary-foreground inline-flex h-8 items-center gap-1 rounded-md px-3 text-xs font-medium"
            onClick={() => setOpen(true)}
          >
            <IconPlus class="size-3.5" />
            Add node
          </button>
        </div>
      </header>

      <p class="text-muted-ink desk-mono text-xs">bound nodes: {nodes().length}</p>

      <div class="flex flex-col gap-3">
        <For each={["macbook-pro", "homelab", "ci-runner"]}>
          {(name) => (
            <article class="bg-surface border-line flex items-center gap-3 rounded-xl border px-4 py-3">
              <IconCircleCheck class="size-4 text-ok" />
              <span class="text-sm font-medium">{name}</span>
              <span class="text-muted-ink desk-mono ml-auto text-xs">nd_{name.slice(0, 4)}</span>
            </article>
          )}
        </For>
      </div>

      <Portal>
        <Dialog isOpen={open()} onClose={() => setOpen(false)}>
          <DialogOverlay class="fixed inset-0 z-40 bg-black/35" />
          <DialogPanel class="bg-popover text-popover-foreground border-line fixed top-1/2 left-1/2 z-50 w-[min(92vw,420px)] -translate-x-1/2 -translate-y-1/2 rounded-xl border p-5 shadow-xl">
            <DialogTitle class="text-base font-semibold">Terracotta is wired</DialogTitle>
            <DialogDescription class="text-muted-ink mt-1 text-xs">
              Focus is trapped, Escape closes, and focus returns to the trigger.
            </DialogDescription>
            <div class="mt-5 flex justify-end">
              <button
                type="button"
                class="border-line hover:bg-chip-hover h-8 rounded-md border px-3 text-xs"
                onClick={() => setOpen(false)}
              >
                {m.common_close()}
              </button>
            </div>
          </DialogPanel>
        </Dialog>
      </Portal>

      <Toaster class="fixed right-4 bottom-4 z-[60] flex flex-col gap-2">
        <For each={queue()}>
          {(item) => (
            <Toast class="bg-popover text-popover-foreground border-line rounded-lg border px-3 py-2 text-xs shadow-lg">
              {item.data}
            </Toast>
          )}
        </For>
      </Toaster>
    </div>
  );
}
