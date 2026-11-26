import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Token contrast guard for the Desk palettes.
 *
 * A selected row has to keep its identity in every palette and both modes.
 * `--signal-soft` is a washed tint and cannot carry that on the light
 * panels, which is why selection owns `--selection`; this test keeps that
 * role from drifting back into "barely visible" when a palette is edited.
 */
const css = readFileSync(resolve(process.cwd(), "src/app.css"), "utf8");

interface Oklch {
  l: number;
  c: number;
  h: number;
}

function parseOklch(value: string): Oklch {
  const match = /^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)$/.exec(value.trim());
  if (match === null) throw new Error(`Unsupported color token value: ${value}`);
  return { l: Number(match[1]), c: Number(match[2]), h: Number(match[3]) };
}

/** OKLCH -> linear sRGB, as defined by CSS Color 4. */
function linearChannels({ l, c, h }: Oklch): [number, number, number] {
  const a = c * Math.cos((h * Math.PI) / 180);
  const b = c * Math.sin((h * Math.PI) / 180);
  const lRoot = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const mRoot = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const sRoot = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    4.0767416621 * lRoot - 3.3077115913 * mRoot + 0.2309699292 * sRoot,
    -1.2684380046 * lRoot + 2.6097574011 * mRoot - 0.3413193965 * sRoot,
    -0.0041960863 * lRoot - 0.7034186147 * mRoot + 1.707614701 * sRoot,
  ];
}

function luminance(color: Oklch): number {
  const [r, g, b] = linearChannels(color).map((channel) => Math.min(1, Math.max(0, channel)));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(first: Oklch, second: Oklch): number {
  const [high, low] = [luminance(first), luminance(second)].toSorted((a, b) => b - a);
  return (high + 0.05) / (low + 0.05);
}

function declaration(block: string, name: string): Oklch {
  const match = new RegExp(`--${name}:\\s*([^;]+);`).exec(block);
  if (match === null) throw new Error(`Missing token --${name}`);
  return parseOklch(match[1]);
}

const paletteBlocks = [
  ...css.matchAll(
    /(:root(?:\[data-palette="[a-z]+"\])?|\.dark(?:\[data-palette="[a-z]+"\])?)\s*\{([^}]*)\}/g,
  ),
]
  .filter(([, , body]) => body.includes("--selection"))
  .map(([, selector, body]) => ({ selector, body }));

describe("Desk palette selection contrast", () => {
  it("declares a selection surface for every palette and mode", () => {
    expect(paletteBlocks.map(({ selector }) => selector)).toEqual([
      ":root",
      ".dark",
      ':root[data-palette="graphite"]',
      '.dark[data-palette="graphite"]',
      ':root[data-palette="moss"]',
      '.dark[data-palette="moss"]',
    ]);
  });

  it.each(paletteBlocks.map(({ selector, body }) => [selector, body] as const))(
    "separates the selected row from the sidebar in %s",
    (selector, body) => {
      const sidebar = declaration(body, "sidebar-bg");
      const selection = declaration(body, "selection");
      const ink = declaration(body, "ink");

      // A fill below this reads as "no fill" next to the panel it sits on.
      expect(
        contrast(selection, sidebar),
        `${selector}: --selection against --sidebar-bg`,
      ).toBeGreaterThanOrEqual(1.2);
      // The stronger fill must not cost the title its text contrast.
      expect(contrast(ink, selection), `${selector}: --ink on --selection`).toBeGreaterThanOrEqual(
        4.5,
      );
    },
  );
});
