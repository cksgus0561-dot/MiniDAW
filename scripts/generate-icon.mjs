import { writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';

// Small code-generated application icon; no image tooling or dependencies required.
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const body = Buffer.concat([Buffer.from(type), data]);
  const length = Buffer.alloc(4);
  const checksum = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  checksum.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, checksum]);
}

function makePng(size) {
  const rows = Buffer.alloc((size * 4 + 1) * size);
  const heights = [0.18, 0.42, 0.66, 0.3, 0.52];
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const nx = x / size;
      const ny = y / size;
      const filled = heights.some((height, i) =>
        Math.abs(nx - (0.23 + i * 0.135)) < 0.034 && Math.abs(ny - 0.5) < height / 2);
      const offset = y * (size * 4 + 1) + 1 + x * 4;
      rows.set(filled ? [128, 223, 188, 255] : [17, 20, 25, 255], offset);
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8;
  header[9] = 6;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', header), chunk('IDAT', deflateSync(rows)), chunk('IEND', Buffer.alloc(0)),
  ]);
}

const images = [32, 256].map(size => ({ size, png: makePng(size) }));
const directory = Buffer.alloc(6 + images.length * 16);
directory.writeUInt16LE(1, 2);
directory.writeUInt16LE(images.length, 4);
let offset = directory.length;
images.forEach(({ size, png }, index) => {
  const entry = 6 + index * 16;
  directory[entry] = size % 256;
  directory[entry + 1] = size % 256;
  directory.writeUInt16LE(1, entry + 4);
  directory.writeUInt16LE(32, entry + 6);
  directory.writeUInt32LE(png.length, entry + 8);
  directory.writeUInt32LE(offset, entry + 12);
  offset += png.length;
});
writeFileSync(new URL('../src-tauri/icons/icon.png', import.meta.url), makePng(128));
writeFileSync(new URL('../src-tauri/icons/icon.ico', import.meta.url), Buffer.concat([directory, ...images.map(image => image.png)]));
console.log('Application icons generated.');
