/**
 * Generate `static/favicon.svg` and the PWA PNGs (192 / 512 / maskable-512)
 * without external dependencies from the shared geometry in `src/lib/brand-icon.ts`.
 *
 * `node scripts/generate-icons.mjs`
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";
import {
  BRAND_ACCENT_BAR,
  BRAND_FG_CIRCLES,
  BRAND_FG_STROKES,
  BRAND_ICON_RADIUS,
  BRAND_ICON_SIZE,
  BRAND_STATIC_COLORS,
  BRAND_STROKE_WIDTH,
  renderBrandFaviconSvg,
} from "../src/lib/brand-icon.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const staticDir = join(root, "static");

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const typeAndData = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typeAndData), 0);
  return Buffer.concat([length, typeAndData, crc]);
}

/** Encode an RGBA raster as a PNG. */
function encodePng(width, height, rgba) {
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y += 1) {
    raw[y * (stride + 1)] = 0; // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type: RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function parseHexColor(hex) {
  const clean = hex.replace("#", "");
  return [
    Number.parseInt(clean.slice(0, 2), 16),
    Number.parseInt(clean.slice(2, 4), 16),
    Number.parseInt(clean.slice(4, 6), 16),
  ];
}

const BG = parseHexColor(BRAND_STATIC_COLORS.bg);
const FG = parseHexColor(BRAND_STATIC_COLORS.fg);
const ACCENT = parseHexColor(BRAND_STATIC_COLORS.accent);

function sampleQuad(x0, y0, cx, cy, x1, y1, steps = 24) {
  const segs = [];
  let px = x0;
  let py = y0;
  for (let i = 1; i <= steps; i += 1) {
    const t = i / steps;
    const u = 1 - t;
    const nx = u * u * x0 + 2 * u * t * cx + t * t * x1;
    const ny = u * u * y0 + 2 * u * t * cy + t * t * y1;
    segs.push([px, py, nx, ny]);
    px = nx;
    py = ny;
  }
  return segs;
}

function sampleCubic(x0, y0, c1x, c1y, c2x, c2y, x1, y1, steps = 24) {
  const segs = [];
  let px = x0;
  let py = y0;
  for (let i = 1; i <= steps; i += 1) {
    const t = i / steps;
    const u = 1 - t;
    const nx = u * u * u * x0 + 3 * u * u * t * c1x + 3 * u * t * t * c2x + t * t * t * x1;
    const ny = u * u * u * y0 + 3 * u * u * t * c1y + 3 * u * t * t * c2y + t * t * t * y1;
    segs.push([px, py, nx, ny]);
    px = nx;
    py = ny;
  }
  return segs;
}

function flattenStrokes(strokes) {
  const segs = [];
  for (const stroke of strokes) {
    let x = 0;
    let y = 0;
    for (const seg of stroke) {
      if (seg.kind === "move") {
        x = seg.x;
        y = seg.y;
      } else if (seg.kind === "line") {
        segs.push([x, y, seg.x, seg.y]);
        x = seg.x;
        y = seg.y;
      } else if (seg.kind === "quad") {
        segs.push(...sampleQuad(x, y, seg.cx, seg.cy, seg.x, seg.y));
        x = seg.x;
        y = seg.y;
      } else if (seg.kind === "cubic") {
        segs.push(...sampleCubic(x, y, seg.c1x, seg.c1y, seg.c2x, seg.c2y, seg.x, seg.y));
        x = seg.x;
        y = seg.y;
      }
    }
  }
  return segs;
}

const STROKE_R = BRAND_STROKE_WIDTH / 2;
const FG_SEGMENTS = flattenStrokes(BRAND_FG_STROKES);
const BAR_R = BRAND_ACCENT_BAR.rx;
const BAR_SEGMENT = [
  BRAND_ACCENT_BAR.x + BAR_R,
  BRAND_ACCENT_BAR.y + BAR_R,
  BRAND_ACCENT_BAR.x + BRAND_ACCENT_BAR.width - BAR_R,
  BRAND_ACCENT_BAR.y + BAR_R,
];

function distToSegment(px, py, ax, ay, bx, by) {
  const vx = bx - ax;
  const vy = by - ay;
  const wx = px - ax;
  const wy = py - ay;
  const lenSq = vx * vx + vy * vy;
  const t = lenSq > 0 ? Math.max(0, Math.min(1, (wx * vx + wy * vy) / lenSq)) : 0;
  return Math.hypot(px - (ax + t * vx), py - (ay + t * vy));
}

function insideRoundedRect(x, y, left, top, right, bottom, radius) {
  if (x < left || x > right || y < top || y > bottom) return false;
  const dx = Math.max(left + radius - x, x - (right - radius), 0);
  const dy = Math.max(top + radius - y, y - (bottom - radius), 0);
  return dx * dx + dy * dy <= radius * radius;
}

function distToForeground(vx, vy) {
  let minD = Infinity;
  for (const { cx, cy, r } of BRAND_FG_CIRCLES) {
    const d = Math.abs(Math.hypot(vx - cx, vy - cy) - r);
    if (d < minD) minD = d;
  }
  for (const [ax, ay, bx, by] of FG_SEGMENTS) {
    const d = distToSegment(vx, vy, ax, ay, bx, by);
    if (d < minD) minD = d;
  }
  return minD - STROKE_R;
}

function distToAccentBar(vx, vy) {
  return distToSegment(vx, vy, ...BAR_SEGMENT) - BAR_R;
}

function renderIcon(size, { maskable }) {
  const rgba = Buffer.alloc(size * size * 4);
  const samples = 4; // 4x4 supersampling for smooth edges
  const radius = maskable ? 0 : size * (BRAND_ICON_RADIUS / BRAND_ICON_SIZE);
  // Maskable icons keep the artwork inside the central 76% safe zone.
  const inset = maskable ? size * 0.12 : 0;
  const inner = size - inset * 2;
  const total = samples * samples;

  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      let bgHits = 0;
      let fgHits = 0;
      let barHits = 0;
      for (let sy = 0; sy < samples; sy += 1) {
        for (let sx = 0; sx < samples; sx += 1) {
          const px = x + (sx + 0.5) / samples;
          const py = y + (sy + 0.5) / samples;
          if (maskable || insideRoundedRect(px, py, 0, 0, size, size, radius)) {
            bgHits += 1;
          }
          const vx = ((px - inset) / inner) * BRAND_ICON_SIZE;
          const vy = ((py - inset) / inner) * BRAND_ICON_SIZE;
          if (vx >= 0 && vx <= BRAND_ICON_SIZE && vy >= 0 && vy <= BRAND_ICON_SIZE) {
            if (distToForeground(vx, vy) <= 0) {
              fgHits += 1;
            } else if (distToAccentBar(vx, vy) <= 0) {
              barHits += 1;
            }
          }
        }
      }
      const offset = (y * size + x) * 4;
      const fgW = fgHits / total;
      const barW = barHits / total;
      const bgW = Math.max(0, 1 - fgW - barW);
      rgba[offset] = Math.round(BG[0] * bgW + FG[0] * fgW + ACCENT[0] * barW);
      rgba[offset + 1] = Math.round(BG[1] * bgW + FG[1] * fgW + ACCENT[1] * barW);
      rgba[offset + 2] = Math.round(BG[2] * bgW + FG[2] * fgW + ACCENT[2] * barW);
      rgba[offset + 3] = Math.round((bgHits / total) * 255);
    }
  }
  return encodePng(size, size, rgba);
}

mkdirSync(staticDir, { recursive: true });
writeFileSync(join(staticDir, "favicon.svg"), renderBrandFaviconSvg());
console.log("wrote static/favicon.svg");

const outputs = [
  ["icon-192.png", 192, { maskable: false }],
  ["icon-512.png", 512, { maskable: false }],
  ["icon-maskable-512.png", 512, { maskable: true }],
];
for (const [name, size, options] of outputs) {
  writeFileSync(join(staticDir, name), renderIcon(size, options));
  console.log(`wrote static/${name}`);
}
