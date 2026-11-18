import { For } from "solid-js";

const SWATCHES = [
  { name: "canvas", className: "bg-canvas" },
  { name: "surface", className: "bg-surface" },
  { name: "chip", className: "bg-chip" },
  { name: "signal", className: "bg-signal" },
  { name: "ok", className: "bg-ok" },
  { name: "warn", className: "bg-warn" },
  { name: "danger", className: "bg-danger" },
  { name: "line", className: "bg-line" },
];

export default function Styleguide() {
  return (
    <div class="desk-page">
      <header class="flex flex-col gap-1">
        <h1 class="text-2xl font-semibold tracking-tight">Styleguide</h1>
        <p class="text-muted-ink text-sm">
          Token and typography check for the ported Desk stylesheet.
        </p>
      </header>

      <section class="flex flex-col gap-3">
        <h2 class="desk-label">Semantic colours</h2>
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <For each={SWATCHES}>
            {(swatch) => (
              <div class="border-line flex flex-col gap-2 rounded-lg border p-2">
                <div class={`h-12 rounded-md ${swatch.className}`} />
                <span class="desk-mono text-muted-ink text-[11px]">{swatch.name}</span>
              </div>
            )}
          </For>
        </div>
      </section>

      <section class="flex flex-col gap-2">
        <h2 class="desk-label">Type scale</h2>
        <p class="text-2xl font-semibold tracking-tight">Sessions across every node</p>
        <p class="text-sm">Body text sits at 0.875rem with a 1.55 line height.</p>
        <p class="text-muted-ink text-xs">Muted secondary text for metadata and hints.</p>
        <p class="desk-mono text-xs">nd_01h8x · v0.4.2 · 3 sessions</p>
      </section>

      <section class="flex flex-col gap-2">
        <h2 class="desk-label">Surfaces</h2>
        <div class="desk-panel p-4">
          <p class="text-sm font-medium">Panel</p>
          <p class="text-muted-ink text-xs">desk-panel: surface, line, radius-lg.</p>
        </div>
      </section>
    </div>
  );
}
