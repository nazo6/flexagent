/**
 * Single source of truth for the FlexAgent (`fxg`) brand mark geometry.
 *
 * Consumed by:
 * - `BrandIcon.svelte` (theme-reactive UI header mark)
 * - `scripts/generate-icons.mjs` (writes `static/favicon.svg` and the PWA PNGs)
 */

export const BRAND_ICON_SIZE = 64;
export const BRAND_ICON_RADIUS = 15;
export const BRAND_STROKE_WIDTH = 4.6;

export const BRAND_STATIC_COLORS = {
  bg: "#24201c",
  fg: "#f7f5f0",
  accent: "#e58835",
} as const;

export type BrandSegment =
  | { kind: "move"; x: number; y: number }
  | { kind: "line"; x: number; y: number }
  | { kind: "quad"; cx: number; cy: number; x: number; y: number }
  | { kind: "cubic"; c1x: number; c1y: number; c2x: number; c2y: number; x: number; y: number };

export type BrandStroke = readonly BrandSegment[];

/** Geometric strokes for the `"fxg"` glyph in `0..64` viewBox units. */
export const BRAND_FG_STROKES: readonly BrandStroke[] = [
  // 'f' stem + top arch
  [
    { kind: "move", x: 14.5, y: 38.5 },
    { kind: "line", x: 14.5, y: 21.5 },
    { kind: "quad", cx: 14.5, cy: 16.3, x: 19.8, y: 16.3 },
    { kind: "line", x: 22, y: 16.3 },
  ],
  // 'f' crossbar
  [
    { kind: "move", x: 11, y: 25 },
    { kind: "line", x: 20.5, y: 25 },
  ],
  // 'x' diagonals
  [
    { kind: "move", x: 25.5, y: 24 },
    { kind: "line", x: 34, y: 38.5 },
  ],
  [
    { kind: "move", x: 34, y: 24 },
    { kind: "line", x: 25.5, y: 38.5 },
  ],
  // 'g' descender stem + tail
  [
    { kind: "move", x: 51, y: 25 },
    { kind: "line", x: 51, y: 39.8 },
    { kind: "cubic", c1x: 51, c1y: 43.2, c2x: 48.6, c2y: 45.2, x: 45, y: 45.2 },
    { kind: "cubic", c1x: 42.8, c1y: 45.2, c2x: 41, c2y: 44.5, x: 39.9, y: 43.5 },
  ],
];

export const BRAND_FG_CIRCLES: readonly { cx: number; cy: number; r: number }[] = [
  { cx: 45, cy: 31, r: 6 },
];

export const BRAND_ACCENT_BAR = {
  x: 11,
  y: 45.5,
  width: 16.5,
  height: 3.8,
  rx: 1.9,
} as const;

function strokeToSvgPath(stroke: BrandStroke): string {
  return stroke
    .map((seg) => {
      switch (seg.kind) {
        case "move":
          return `M${seg.x} ${seg.y}`;
        case "line":
          return `L${seg.x} ${seg.y}`;
        case "quad":
          return `Q${seg.cx} ${seg.cy} ${seg.x} ${seg.y}`;
        case "cubic":
          return `C${seg.c1x} ${seg.c1y} ${seg.c2x} ${seg.c2y} ${seg.x} ${seg.y}`;
      }
    })
    .join(" ");
}

export const BRAND_FG_PATH_STRINGS: readonly string[] = BRAND_FG_STROKES.map(strokeToSvgPath);

/** Render the standalone static SVG (used for `static/favicon.svg`). */
export function renderBrandFaviconSvg(): string {
  const paths = BRAND_FG_PATH_STRINGS.map((d) => `    <path d="${d}" />`).join("\n");
  const circles = BRAND_FG_CIRCLES.map(
    ({ cx, cy, r }) => `    <circle cx="${cx}" cy="${cy}" r="${r}" />`,
  ).join("\n");
  const bar = BRAND_ACCENT_BAR;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${BRAND_ICON_SIZE} ${BRAND_ICON_SIZE}">
  <rect width="${BRAND_ICON_SIZE}" height="${BRAND_ICON_SIZE}" rx="${BRAND_ICON_RADIUS}" fill="${BRAND_STATIC_COLORS.bg}" />
  <g fill="none" stroke="${BRAND_STATIC_COLORS.fg}" stroke-width="${BRAND_STROKE_WIDTH}" stroke-linecap="round">
${paths}
${circles}
  </g>
  <rect x="${bar.x}" y="${bar.y}" width="${bar.width}" height="${bar.height}" rx="${bar.rx}" fill="${BRAND_STATIC_COLORS.accent}" />
</svg>
`;
}
