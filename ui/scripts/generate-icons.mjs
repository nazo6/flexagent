/**
 * PWA アイコン (192 / 512 / maskable-512) を依存なしで生成する。
 *
 * `node scripts/generate-icons.mjs` で `static/` に PNG を出力する。
 * ロゴは `static/favicon.svg` と同じ「fxg」を抽象化したマーク
 * (濃紺の角丸背景 + ライトブルーの F 字バー)。
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

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

/** RGBA ラスタを PNG へエンコードする。 */
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

const BG = [15, 23, 42]; // #0f172a
const ACCENT = [125, 211, 252]; // #7dd3fc

/** 角丸四角形の内部判定 (px 座標)。 */
function insideRoundedRect(x, y, left, top, right, bottom, radius) {
  if (x < left || x > right || y < top || y > bottom) return false;
  const dx = Math.max(left + radius - x, x - (right - radius), 0);
  const dy = Math.max(top + radius - y, y - (bottom - radius), 0);
  return dx * dx + dy * dy <= radius * radius;
}

/** F 字マーク (縦棒 + 上横棒 + 中横棒) の内部判定。 */
function insideMark(x, y, size) {
  const bars = [
    [0.3, 0.2, 0.42, 0.8], // 縦棒
    [0.3, 0.2, 0.74, 0.34], // 上横棒
    [0.3, 0.46, 0.66, 0.6], // 中横棒
  ];
  return bars.some(([l, t, r, b]) => {
    const left = l * size;
    const top = t * size;
    const right = r * size;
    const bottom = b * size;
    return x >= left && x <= right && y >= top && y <= bottom;
  });
}

function renderIcon(size, { maskable }) {
  const rgba = Buffer.alloc(size * size * 4);
  const samples = 3; // 3x3 スーパーサンプリングでエッジを滑らかにする
  const radius = maskable ? 0 : size * 0.22;
  const inset = maskable ? size * 0.1 : 0;
  const inner = size - inset * 2;

  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      let bgHits = 0;
      let markHits = 0;
      for (let sy = 0; sy < samples; sy += 1) {
        for (let sx = 0; sx < samples; sx += 1) {
          const px = x + (sx + 0.5) / samples;
          const py = y + (sy + 0.5) / samples;
          if (maskable || insideRoundedRect(px, py, 0, 0, size - 1, size - 1, radius)) {
            bgHits += 1;
          }
          const mx = (px - inset) / inner;
          const my = (py - inset) / inner;
          if (mx >= 0 && mx <= 1 && my >= 0 && my <= 1 && insideMark(mx * size, my * size, size)) {
            markHits += 1;
          }
        }
      }
      const total = samples * samples;
      const offset = (y * size + x) * 4;
      const bgAlpha = (bgHits / total) * 255;
      const markAlpha = markHits / total;
      const blend = (base, accent) => Math.round(base + (accent - base) * markAlpha);
      rgba[offset] = blend(BG[0], ACCENT[0]);
      rgba[offset + 1] = blend(BG[1], ACCENT[1]);
      rgba[offset + 2] = blend(BG[2], ACCENT[2]);
      rgba[offset + 3] = Math.round(bgAlpha);
    }
  }
  return encodePng(size, size, rgba);
}

mkdirSync(staticDir, { recursive: true });
const outputs = [
  ["icon-192.png", 192, { maskable: false }],
  ["icon-512.png", 512, { maskable: false }],
  ["icon-maskable-512.png", 512, { maskable: true }],
];
for (const [name, size, options] of outputs) {
  writeFileSync(join(staticDir, name), renderIcon(size, options));
  console.log(`wrote static/${name}`);
}
