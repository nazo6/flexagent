import {
  BRAND_ACCENT_BAR,
  BRAND_FG_CIRCLES,
  BRAND_FG_PATH_STRINGS,
  BRAND_ICON_RADIUS,
  BRAND_ICON_SIZE,
  BRAND_STROKE_WIDTH,
} from "#lib/brand-icon.ts";
import { cn } from "#lib/utils.ts";

/** Brand mark. Size and layout classes go on the root `<svg>`. */
export default function BrandIcon(props: { class?: string }) {
  const borderInset = 0.75;
  return (
    <svg
      viewBox={`0 0 ${BRAND_ICON_SIZE} ${BRAND_ICON_SIZE}`}
      aria-hidden="true"
      class={cn("shrink-0", props.class ?? "size-7")}
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
        class="stroke-ink"
        stroke-width={BRAND_STROKE_WIDTH}
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        {BRAND_FG_PATH_STRINGS.map((d) => (
          <path d={d} />
        ))}
        {BRAND_FG_CIRCLES.map((circle) => (
          <circle cx={circle.cx} cy={circle.cy} r={circle.r} />
        ))}
      </g>
      <rect
        width={BRAND_ACCENT_BAR.width}
        height={BRAND_ACCENT_BAR.height}
        x={BRAND_ACCENT_BAR.x}
        y={BRAND_ACCENT_BAR.y}
        rx={BRAND_ACCENT_BAR.rx}
        class="fill-signal"
      />
    </svg>
  );
}
