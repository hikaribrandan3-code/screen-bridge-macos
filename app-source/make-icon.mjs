// Generates app-icon.png (1024x1024): Apple-blue rounded square with a
// Mac screen + tablet glyph, pure Node (no deps).
import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';

const S = 1024;
const px = new Uint8Array(S * S * 4);

const inRoundRect = (x, y, rx, ry, rw, rh, r) => {
  if (x < rx || x >= rx + rw || y < ry || y >= ry + rh) return false;
  const cx = Math.max(rx + r, Math.min(x, rx + rw - r));
  const cy = Math.max(ry + r, Math.min(y, ry + rh - r));
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r || (x >= rx + r && x < rx + rw - r) || (y >= ry + r && y < ry + rh - r);
};

for (let y = 0; y < S; y++) {
  for (let x = 0; x < S; x++) {
    const i = (y * S + x) * 4;
    // background: rounded square, vertical blue gradient #2F97FF -> #0060DF
    if (inRoundRect(x, y, 64, 64, 896, 896, 200)) {
      const t = (y - 64) / 896;
      px[i] = Math.round(0x2f + (0x00 - 0x2f) * t);
      px[i + 1] = Math.round(0x97 + (0x60 - 0x97) * t);
      px[i + 2] = Math.round(0xff + (0xdf - 0xff) * t);
      px[i + 3] = 255;
    }
    // Mac screen (left, larger)
    const mac = inRoundRect(x, y, 200, 330, 400, 290, 28) && !inRoundRect(x, y, 228, 358, 344, 234, 14);
    // tablet (right, overlapping)
    const tab = inRoundRect(x, y, 620, 380, 210, 300, 30) && !inRoundRect(x, y, 644, 404, 162, 252, 14);
    // bridge bar between them
    const bar = inRoundRect(x, y, 560, 480, 100, 44, 22);
    if (mac || tab || bar) {
      px[i] = 255; px[i + 1] = 255; px[i + 2] = 255; px[i + 3] = 255;
    }
  }
}

// PNG encode
const raw = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0;
  Buffer.from(px.buffer, y * S * 4, S * 4).copy(raw, y * (S * 4 + 1) + 1);
}
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
writeFileSync('app-icon.png', png);
console.log('wrote app-icon.png');
