<script lang="ts">
  import {
    BRAND_ACCENT_BAR,
    BRAND_FG_CIRCLES,
    BRAND_FG_PATH_STRINGS,
    BRAND_ICON_RADIUS,
    BRAND_ICON_SIZE,
    BRAND_STROKE_WIDTH,
  } from "#lib/brand-icon.ts";
  import { cn } from "#lib/utils.ts";

  let {
    class: className = "size-7",
  }: {
    /** Size and layout classes applied to the root `<svg>`. */
    class?: string;
  } = $props();

  const borderInset = 0.75;
</script>

<svg
  viewBox={`0 0 ${BRAND_ICON_SIZE} ${BRAND_ICON_SIZE}`}
  aria-hidden="true"
  class={cn("shrink-0", className)}
>
  <rect
    x={borderInset}
    y={borderInset}
    width={BRAND_ICON_SIZE - borderInset * 2}
    height={BRAND_ICON_SIZE - borderInset * 2}
    rx={BRAND_ICON_RADIUS - borderInset}
    stroke-width={borderInset * 2}
    class="fill-ink stroke-transparent dark:fill-surface-raised dark:stroke-line-strong"
  />
  <g
    fill="none"
    class="stroke-canvas dark:stroke-ink"
    stroke-width={BRAND_STROKE_WIDTH}
    stroke-linecap="round"
  >
    {#each BRAND_FG_PATH_STRINGS as d (d)}
      <path {d} />
    {/each}
    {#each BRAND_FG_CIRCLES as circle (`${circle.cx},${circle.cy}`)}
      <circle cx={circle.cx} cy={circle.cy} r={circle.r} />
    {/each}
  </g>
  <rect
    x={BRAND_ACCENT_BAR.x}
    y={BRAND_ACCENT_BAR.y}
    width={BRAND_ACCENT_BAR.width}
    height={BRAND_ACCENT_BAR.height}
    rx={BRAND_ACCENT_BAR.rx}
    class="brand-signal-bar"
  />
</svg>

<style>
  .brand-signal-bar {
    fill: color-mix(in oklch, var(--signal) 72%, white);
  }

  :global(.dark) .brand-signal-bar {
    fill: var(--signal);
  }
</style>
