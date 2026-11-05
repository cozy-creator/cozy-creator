// What the web proofs share: real media made here, a timeout on every wait, and a drop as the browser delivers one.
const { writeFileSync } = require('fs');
const zlib = require('zlib');

const STEP_MS = 60_000;
// Past live.js's first run, the longest any cell waits between two checks.
const STALL_MS = 8 * 60_000;
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

// A real video: one second of H.264, 160 by 90, no sound.
const MP4 = Buffer.from('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', 'base64');

// A real picture: `shade` tells one from another, so each hashes differently. With `alpha`, its left half is fully transparent.
function png(shade, width = 320, height = 200, alpha = false) {
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
  header[9] = alpha ? 6 : 2;
  const rows = Buffer.alloc((width * (alpha ? 4 : 3) + 1) * height);
  for (let y = 0, at = 0; y < height; y++) {
    rows[at++] = 0;
    for (let x = 0; x < width; x++) {
      rows[at++] = shade;
      rows[at++] = (x * 255 / width) | 0;
      rows[at++] = (y * 255 / height) | 0;
      if (alpha) rows[at++] = x < width / 2 ? 0 : 255;
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

// The pointer on it, as a person's. A locator's hover that retries scrolls the canvas to centre what it's after, and React Flow scrolls back within
// milliseconds, which leaves the pointer over another box: that box opens, and this one closes (macOS, 41927957996: `web-nodes-died.json`).
async function pointAt(page, locator) {
  const { x, y, width, height } = await locator.boundingBox();
  await page.mouse.move(x + width / 2, y + height / 2);
}

// A box's controls open under it and may pass the window's edge. The canvas doesn't scroll for them, so this takes it up by the wheel, as a person
// does, and comes back to the box.
async function inView(page, box) {
  const controls = await box.locator('.asset-more').boundingBox({ timeout: 500 }).catch(() => null);
  const over = controls ? controls.y + controls.height + 16 - page.viewportSize().height : 0;
  if (over <= 0) return;
  await page.mouse.wheel(0, over);
  await page.waitForTimeout(250);
  await pointAt(page, box.locator('.nm'));
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

// The tab a proof is on, and what it looked like when the proof died: a thrown wait says only what it waited for.
let looking;
const lookAt = (page) => { looking = page; };
// What opens and closes boxes, in order, in the page: the pointer between them, focus, a box or the canvas moving, a node coming or going.
// For a tab that isn't measuring anything's speed.
const watchBoxes = (page) => page.addInitScript(() => {
  const log = (window.boxLog = []);
  const say = (line) => {
    log.push(`${Math.round(performance.now())} ${line}`);
    if (log.length > 300) log.shift();
  };
  const boxOf = (el) => el?.closest?.('.react-flow__node')?.querySelector('.card')?.dataset.asset ?? null;
  let under = null;
  document.addEventListener('pointerover', ({ target, clientX, clientY }) => {
    const now = boxOf(target);
    if (now !== under) say(`pointer ${under} -> ${now} at ${clientX},${clientY}`);
    under = now;
  }, true);
  for (const type of ['focusin', 'focusout']) {
    document.addEventListener(type, ({ target }) => say(`${type} ${boxOf(target)} ${target.tagName}.${target.className} visible=${target.matches(':focus-visible')}`), true);
  }
  document.addEventListener('scroll', ({ target }) => say(`scrolled ${target.className ?? 'document'} to ${target.scrollLeft},${target.scrollTop}`), true);
  new MutationObserver((records) => {
    for (const { type, target, attributeName, oldValue, addedNodes, removedNodes } of records) {
      for (const node of addedNodes) if (node.classList?.contains('react-flow__node')) say(`added ${boxOf(node)} ${node.style.transform}`);
      for (const node of removedNodes) if (node.classList?.contains('react-flow__node')) say(`removed ${node.dataset.id}`);
      if (type !== 'attributes') continue;
      if (attributeName === 'class' && target.classList.contains('card')) {
        const [was, is] = [/\bactive\b/.test(oldValue ?? ''), target.classList.contains('active')];
        if (was !== is) say(`${target.dataset.asset} ${is ? 'opens' : 'closes'}`);
      }
      if (attributeName === 'style' && !oldValue?.includes(target.style.transform)) {
        if (target.classList.contains('react-flow__viewport')) say(`canvas moved to ${target.style.transform}`);
        if (target.classList.contains('react-flow__node')) say(`${boxOf(target)} moved to ${target.style.transform}`);
      }
    }
  }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['class', 'style'], attributeOldValue: true });
});
const boxLog = (page) => page.evaluate(() => ({
  log: window.boxLog?.slice(-40),
  focus: `${document.activeElement?.tagName}.${document.activeElement?.className}`,
  under: [...document.querySelectorAll(':hover')].at(-1)?.className,
}));
async function lastLook() {
  if (!looking || !process.env.SHOT) return;
  await looking.screenshot({ path: `${process.env.SHOT}-died.png`, timeout: 10_000 }).catch(() => {});
  const told = boxLog(looking).then((said) => writeFileSync(`${process.env.SHOT}-died.json`, JSON.stringify(said, null, 2))).catch(() => {});
  await Promise.race([told, new Promise((done) => setTimeout(done, 10_000))]);
}

/** `died`, the cell's own ending, once `seen` stops growing: a wait on a page or a command that never answers ends no other way. */
function noStall(seen, died) {
  let [held, since] = [-1, Date.now()];
  const watch = setInterval(() => {
    const now = Object.keys(seen).length;
    if (now !== held) [held, since] = [now, Date.now()];
    else if (Date.now() - since > STALL_MS) {
      clearInterval(watch);
      died(new Error(`nothing new for ${STALL_MS} ms, after ${Object.keys(seen).at(-1)}`));
    }
  }, 5000);
  watch.unref();
}

module.exports = { MP4, STEP_MS, boxLog, drop, inView, lastLook, lookAt, noStall, png, pointAt, until, watchBoxes, wav };
