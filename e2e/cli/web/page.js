// What the web proofs share: real media made here, a timeout on every wait, and a drop as the browser delivers one.
const zlib = require('zlib');

const STEP_MS = 60_000;
const crc = (() => {
  const table = Array.from({ length: 256 }, (_, n) => {
    for (let k = 0; k < 8; k++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1;
    return n >>> 0;
  });
  return (bytes) => {
    let c = 0xffffffff;
    for (const b of bytes) c = table[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
})();

// A real picture: `shade` tells one from another, so each hashes differently.
function png(shade, width = 320, height = 200) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const length = Buffer.alloc(4), sum = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const rows = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0, at = 0; y < height; y++) {
    rows[at++] = 0;
    for (let x = 0; x < width; x++) {
      rows[at++] = shade;
      rows[at++] = (x * 255 / width) | 0;
      rows[at++] = (y * 255 / height) | 0;
    }
  }
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', zlib.deflateSync(rows)), chunk('IEND', Buffer.alloc(0))]);
}

function wav(hz = 440, seconds = 1, rate = 8000) {
  const count = seconds * rate, data = Buffer.alloc(count * 2);
  for (let i = 0; i < count; i++) data.writeInt16LE(Math.round(Math.sin(2 * Math.PI * hz * i / rate) * 12000), i * 2);
  const head = Buffer.alloc(44);
  head.write('RIFF', 0); head.writeUInt32LE(36 + data.length, 4); head.write('WAVEfmt ', 8); head.writeUInt32LE(16, 16);
  head.writeUInt16LE(1, 20); head.writeUInt16LE(1, 22); head.writeUInt32LE(rate, 24); head.writeUInt32LE(rate * 2, 28);
  head.writeUInt16LE(2, 32); head.writeUInt16LE(16, 34); head.write('data', 36); head.writeUInt32LE(data.length, 40);
  return Buffer.concat([head, data]);
}

async function until(what, read, wanted, ms = STEP_MS) {
  const deadline = Date.now() + ms;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (wanted(last)) return last;
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`${what}: still ${JSON.stringify(last)} after ${ms} ms`);
}

async function drop(page, files, x, y) {
  const transfer = await page.evaluateHandle((list) => {
    const data = new DataTransfer();
    for (const file of list) {
      data.items.add(new File([Uint8Array.from(atob(file.b64), (c) => c.charCodeAt(0))], file.name, { type: file.type }));
    }
    return data;
  }, files.map((file) => ({ name: file.name, type: file.type, b64: file.bytes.toString('base64') })));
  for (const type of ['dragenter', 'dragover', 'drop']) {
    await page.dispatchEvent('.stage', type, { dataTransfer: transfer, clientX: x, clientY: y });
  }
}

module.exports = { STEP_MS, drop, png, until, wav };
