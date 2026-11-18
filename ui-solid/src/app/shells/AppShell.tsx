import type { JSX } from "@solidjs/web";
import IconPalette from "~icons/lucide/palette";
import IconServer from "~icons/lucide/server";
import { paths } from "../router";

const navLink =
  "text-muted-ink hover:bg-chip-hover aria-[current=page]:bg-chip aria-[current=page]:text-ink flex items-center gap-2 rounded-md px-2 py-1.5 text-sm";

export default function AppShell(props: { children?: JSX.Element }) {
  return (
    <div class="bg-canvas text-ink flex min-h-screen">
      <aside class="bg-sidebar border-line hidden w-52 shrink-0 flex-col gap-1 border-r p-3 sm:flex">
        <div class="mb-3 px-2 text-sm font-semibold">fxg</div>
        <a href={paths()} class={navLink}>
          <IconServer class="size-4" />
          Nodes
        </a>
        <a href={paths.styleguide} class={navLink}>
          <IconPalette class="size-4" />
          Styleguide
        </a>
        <div class="text-muted-ink mt-auto px-2 text-[11px]">Solid 2 · Phase 0</div>
      </aside>
      <main class="min-w-0 flex-1">{props.children}</main>
    </div>
  );
}
