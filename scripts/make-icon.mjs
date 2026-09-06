#!/usr/bin/env node
/**
 * make-icon.mjs —— 生成 1024x1024 占位 logo.png（深色圆角方块 + 蓝色标记）。
 * 之后可用官方黑鲸鱼 logo 替换：覆盖本文件输出的 logo.png 后重跑
 *   npx @tauri-apps/cli icon logo.png
 */
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 1024;
const px = new Uint8Array(S * S * 4);

const BG = [11, 14, 20, 255];    // #0B0E14 深底
const ACCENT = [79, 110, 247, 255]; // #4F6EF7 蓝
const WHITE = [235, 240, 255, 255];

const set = (x, y, c) => {
  const i = (y * S + x) * 4;
  px[i] = c[0]; px[i + 1] = c[1]; px[i + 2] = c[2]; px[i + 3] = c[3];
};

const inRounded = (x, y, x0, y0, x1, y1, r) => {
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  const cx = Math.max(x0 + r, Math.min(x, x1 - r));
  const cy = Math.max(y0 + r, Math.min(y, y1 - r));
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r || (x >= x0 + r && x <= x1 - r) || (y >= y0 + r && y <= y1 - r);
};

// 外层圆角方块（圆角 224 ≈ 22%）
for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    set(x, y, inRounded(x, y, 0, 0, S - 1, S - 1, 224) ? BG : [0, 0, 0, 0]);
  }
}

// 蓝色圆（主体）
const cx = 512, cy = 470, R = 250;
for (let y = cy - R; y <= cy + R; y++) {
  for (let x = cx - R; x <= cx + R; x++) {
    if ((x - cx) ** 2 + (y - cy) ** 2 <= R * R) set(x, y, ACCENT);
  }
}
// 白色小圆（眼），带抗锯齿近似
const ex = cx + 95, ey = cy - 70, ER = 62;
for (let y = ey - ER - 1; y <= ey + ER + 1; y++) {
  for (let x = ex - ER - 1; x <= ex + ER + 1; x++) {
    const d = Math.sqrt((x - ex) ** 2 + (y - ey) ** 2);
    if (d <= ER) set(x, y, WHITE);
  }
}
// 下方三点头（loading 呼应壳启动页）
const dots = [[512 - 150, 830], [512, 830], [512 + 150, 830]];
for (const [dx, dy] of dots) {
  for (let y = dy - 34; y <= dy + 34; y++) {
    for (let x = dx - 34; x <= dx + 34; x++) {
      if ((x - dx) ** 2 + (y - dy) ** 2 <= 34 ** 2) set(x, y, ACCENT);
    }
  }
}

// ── PNG 编码（IHDR + IDAT + IEND）──
const crcTable = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});
const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
const raw = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0; // filter: none
  Buffer.from(px.buffer, y * S * 4, S * 4).copy(raw, y * (S * 4 + 1) + 1);
}
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync(new URL("../logo.png", import.meta.url), png);
console.log("[icon] logo.png (1024x1024) 已生成");
