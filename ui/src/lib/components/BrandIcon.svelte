<script lang="ts">
  import {
    BRAND_ACCENT_BAR,
    BRAND_FG_CIRCLES,
    BRAND_FG_PATH_STRINGS,
    BRAND_ICON_RADIUS,
    BRAND_ICON_SIZE,
    BRAND_STROKE_WIDTH,
  } from "#lib/brand-icon.ts";
  import { motion } from "#lib/motion.svelte.ts";
  import { cn } from "#lib/utils.ts";
  import { onMount } from "svelte";

  let {
    class: className = "size-7",
    draw = false,
  }: {
    /** Size and layout classes applied to the root `<svg>`. */
    class?: string;
    /** Draw the strokes once; only used by the cold-start loading screen. */
    draw?: boolean;
  } = $props();

  const borderInset = 0.75;

  let svg = $state<SVGSVGElement | null>(null);

  onMount(() => {
    if (!draw || svg === null) return;
    const group = svg.querySelector<SVGGElement>(".brand-strokes");
    const bar = svg.querySelector<SVGRectElement>("[data-brand-bar]");
    if (group === null || bar === null) return;

    if (motion.isReducedNow() || typeof bar.animate !== "function") {
      group.style.opacity = "1";
      bar.style.opacity = "1";
      return;
    }

    const animations: Animation[] = [];
    const strokes = Array.from(svg.querySelectorAll<SVGGeometryElement>("[data-brand-stroke]"));
    strokes.forEach((element, index) => {
      const length = typeof element.getTotalLength === "function" ? element.getTotalLength() : 40;
      element.setAttribute("stroke-dasharray", String(length));
      element.setAttribute("stroke-dashoffset", String(length));
      animations.push(
        element.animate([{ strokeDashoffset: length }, { strokeDashoffset: 0 }], {
          duration: 600,
          delay: index * 35,
          easing: "cubic-bezier(0.2, 0, 0, 1)",
          fill: "forwards",
        }),
      );
    });
    group.style.opacity = "1";
    bar.style.opacity = "0";
    animations.push(
      bar.animate(
        [
          { opacity: 0, transform: "translateX(-10px)" },
          { opacity: 1, transform: "translateX(0)" },
        ],
        {
          duration: 240,
          delay: 500,
          easing: "cubic-bezier(0.34, 1.4, 0.64, 1)",
          fill: "both",
        },
      ),
    );

    return () => {
      for (const animation of animations) animation.cancel();
    };
  });
</script>

<svg
  bind:this={svg}
  viewBox={`0 0 ${BRAND_ICON_SIZE} ${BRAND_ICON_SIZE}`}
  aria-hidden="true"
  class={cn("shrink-0", draw && "brand-draw", className)}
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
    class="brand-strokes stroke-canvas dark:stroke-ink"
    stroke-width={BRAND_STROKE_WIDTH}
    stroke-linecap="round"
  >
    {#each BRAND_FG_PATH_STRINGS as d (d)}
      <path {d} data-brand-stroke />
    {/each}
    {#each BRAND_FG_CIRCLES as circle (`${circle.cx},${circle.cy}`)}
      <circle cx={circle.cx} cy={circle.cy} r={circle.r} data-brand-stroke />
    {/each}
  </g>
  <rect
    x={BRAND_ACCENT_BAR.x}
    y={BRAND_ACCENT_BAR.y}
    width={BRAND_ACCENT_BAR.width}
    height={BRAND_ACCENT_BAR.height}
    rx={BRAND_ACCENT_BAR.rx}
    data-brand-bar
    class="brand-signal-bar"
  />
</svg>

<style>
  .brand-signal-bar {
    fill: color-mix(in oklch, var(--signal) 72%, white);
    transform-box: fill-box;
  }

  :global(.dark) .brand-signal-bar {
    fill: var(--signal);
  }

  /* The draw starts from a hidden glyph, so the first paint shows only the
     rounded tile. `onMount` reveals the strokes once lengths are measured. */
  :global(.brand-draw .brand-strokes) {
    opacity: 0;
  }

  :global(.brand-draw .brand-signal-bar) {
    opacity: 0;
  }
</style>
