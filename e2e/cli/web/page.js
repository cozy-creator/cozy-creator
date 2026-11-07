// What the web proofs share: real media made here, a timeout on every wait, and a drop as the browser delivers one.
const { execFileSync } = require('child_process');
const { appendFileSync, readFileSync, writeFileSync } = require('fs');
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
// Two seconds of colour bars, black for the first half second: a poster from its first frame is black.
const DARK_OPEN = Buffer.from('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', 'base64');

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

// The canvas still: drawn in on a box or gone back, a person reaches for what's there once it stops moving. Still across three looks, as a slow
// runner can draw one frame of a move in longer than a look.
async function settled(page) {
  const at = () => page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform);
  for (let last = await at(), still = 0; still < 3; ) {
    await page.waitForTimeout(100);
    const now = await at();
    still = now === last ? still + 1 : 0;
    last = now;
  }
}

// A box as a person reaches it, worked on or at rest: it's always on the canvas.
const boxOf = (page, name) => page.locator(`.react-flow__node:has(.card[data-asset="${name}"])`).first();
async function closeBox(page) {
  // By the address, as the page has it: after a reload the box is on its way, and not there to count yet, and `page.url()` hears of a move a moment late.
  // Left already, by a click off it say, the canvas may still be on its way back.
  if (!(await page.evaluate(() => location.hash.includes('/assets/')))) return settled(page);
  await page.locator('.card.here').waitFor();
  await page.keyboard.press('Escape');
  await page.locator('.card.here').waitFor({ state: 'detached' });
  await settled(page);
}
// A click works on a box where it stands: the middle of it, where there's nothing else to press.
/** An open box's Render. An image tool's box looking at a take shows only its description: its + goes to its next take's words, with Render. */
async function renderOf(page, name) {
  const here = page.locator(`.card.here[data-asset="${name}"]`).first();
  const next = here.locator('.here-side .take.add[aria-label="Its next take"]');
  if (!(await here.locator('.in-frame').count()) && (await next.count())) await next.click();
  return here.locator('button[aria-label=Render]');
}

async function openBox(page, name) {
  // By the address too: a box left a moment ago can still say so, and reaching for it finds it gone (macOS, 41977214625).
  const card = page.locator(`.card.here[data-asset="${name}"]`);
  if (await page.evaluate((asked) => location.hash.endsWith(`/assets/${encodeURIComponent(asked)}`), name)) return card.waitFor();
  await closeBox(page);
  // After a drag d3-drag swallows clicks at the window until its zero timer, which a cell can beat and a hand can't. This timer runs after that one.
  await page.evaluate(() => new Promise((done) => setTimeout(done, 0)));
  const media = page.locator(`.react-flow__node .card[data-asset="${name}"] .media`).first();
  const middle = async () => {
    const { x, y, width, height } = await media.boundingBox();
    return { x: x + width / 2, y: y + height / 2 };
  };
  const reached = async () => (await media.count()) > 0 && page.evaluate(({ x, y, name }) => document.elementFromPoint(x, y)?.closest('.card')?.dataset.asset === name, { ...(await middle()), name });
  // A box just made takes a moment to draw. Off the view it isn't drawn, or it's under what floats over it: F brings it all in, as for a person.
  await media.waitFor({ state: 'attached', timeout: 1500 }).catch(() => {});
  if (!(await reached())) {
    await page.keyboard.press('f');
    await page.waitForTimeout(300);
    await until(`${name} in reach`, reached, Boolean);
  }
  const { x, y } = await middle();
  const before = await page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform);
  await page.mouse.click(x, y);
  await card.waitFor();
  // The canvas draws in on it: a person reaches for it once it's there. Its move starts a frame after the click, which a slow runner draws late.
  await until(`the canvas drawing in on ${name}`, () => page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform), (now) => now !== before, 2000).catch(() => {});
  await settled(page);
}

// A point on open canvas, clear of every box and what floats over it: where a take dragged out lands as a box of its own.
const openCanvas = (page) => page.evaluate(() => {
  for (let y = innerHeight - 60; y > 120; y -= 30) for (let x = innerWidth - 60; x > 320; x -= 30) if (document.elementFromPoint(x, y)?.classList.contains('react-flow__pane')) return { x, y };
  throw new Error('no open canvas to drop on');
});

// Carried as a hand carries one out of a list: pressed, moved a little across the list first (`across`, its other way), which tells a carry from a
// scroll, then to `to` on the screen, and let go there.
async function carry(page, from, to, across = 'x') {
  const at = await from.boundingBox();
  const start = { x: at.x + at.width / 2, y: at.y + at.height / 2 };
  const lean = across === 'x' ? { x: Math.sign(to.x - start.x) || 1, y: 0 } : { x: 0, y: Math.sign(to.y - start.y) || 1 };
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + lean.x * 12, start.y + lean.y * 12, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 8 });
  await page.mouse.up();
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
  for (const type of ['mousedown', 'mouseup', 'click']) {
    document.addEventListener(type, ({ target }) => say(`${type} ${target.tagName}.${target.className}`), true);
  }
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
/** This computer's Calcifer, where its lock ($HOLDER) says: what serves each cell's page, unless $PAGE_ORIGIN does. */
const calcifer = () => process.env.PAGE_ORIGIN ?? `http://${JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).address}`;

/** `page`, signed in as a person is: `cozy open --no-open`, and its link at once. Its code lives 60 s, so a cell asks for it with its browser
 *  up: asked for before a slow runner launched one, a code had run out by the time the page redeemed it (42113601659). */
function signIn(page, options) {
  const words = ['open', '--no-open'];
  if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
  const said = execFileSync(process.env.COZY, words, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  const link = said.match(/http:\/\/\S+#code=[0-9a-f]+\S*/)?.[0];
  if (!link) throw new Error(`cozy open --no-open printed no link: ${said.slice(0, 300)}`);
  // $PAGE_ORIGIN: the page from elsewhere (a `vite` of the tree under test), its API through it.
  return page.goto(process.env.PAGE_ORIGIN ? link.replace(/^http:\/\/[^/]+/, process.env.PAGE_ORIGIN) : link, options);
}

/** Chrome with its scrollbars shown, as on a person's screen, not hidden as headless has them. */
const BARS = { ignoreDefaultArgs: ['--hide-scrollbars'] };
/** And rastered on the GPU path (SwiftShader where there's no GPU), as a person's Chrome rasters. Headless rasters in software, which drew right what a
 *  person's drew wrong: a drawn edge's tiled sides, shifted out at a fractional scale. Its compositor holds CSS animations at their start. */
const GPU = { ...BARS, args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--enable-gpu-rasterization', '--ignore-gpu-blocklist'] };
/** This pass's browser: $ENGINE's own build (webkit, firefox), else the Chrome at $CHROME, launched with `options`. */
const launch = (playwright, options = {}) => (process.env.ENGINE ? playwright[process.env.ENGINE].launch({ headless: true }) : playwright.chromium.launch({ executablePath: process.env.CHROME, headless: true, ...options }));

async function lastLook() {
  if (!looking || !process.env.LAST_LOOK) return;
  await looking.screenshot({ path: `${process.env.LAST_LOOK}-died.png`, timeout: 10_000 }).catch(() => {});
  const told = boxLog(looking).then(async (said) => {
    const clicks = await looking.evaluate(() => window.clicks?.slice(-12));
    writeFileSync(`${process.env.LAST_LOOK}-died.json`, JSON.stringify({ url: looking.url(), clicks, ...said }, null, 2));
  }).catch(() => {});
  await Promise.race([told, new Promise((done) => setTimeout(done, 10_000))]);
}

/** `died`, the cell's own ending, once `seen` stops growing: a wait on a page or a command that never answers ends no other way. */
function noStall(seen, died) {
  let [held, since] = [-1, Date.now()];
  const watch = setInterval(() => {
    // What it's seen, and how many looks a cell that counts them (seen.looks) has taken.
    const now = Object.keys(seen).length + (seen.looks ?? 0);
    if (now !== held) [held, since] = [now, Date.now()];
    else if (Date.now() - since > STALL_MS) {
      clearInterval(watch);
      died(new Error(`nothing new for ${STALL_MS} ms, after ${Object.keys(seen).at(-1)}`));
    }
  }, 5000);
  watch.unref();
}

/** The frames while a pan goes on, how many places the viewport went, as a pan that didn't move proves nothing, and the fewest cards on
 *  screen at any moment of it. It goes 480 px left and back. */
const panning = (page, ms) => page.evaluate((ms) => new Promise((done) => {
  const pane = document.querySelector('.react-flow__pane');
  const viewport = document.querySelector('.react-flow__viewport');
  const frames = [];
  const places = new Set();
  let [last, sign, count, fewest] = [performance.now(), 1, 0, Infinity];
  const end = last + ms;
  const step = (now) => {
    frames.push(now - last);
    last = now;
    places.add(getComputedStyle(viewport).transform);
    fewest = Math.min(fewest, document.querySelectorAll('.react-flow__node').length);
    if (++count % 40 === 0) sign = -sign;
    pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, bubbles: true, cancelable: true }));
    if (now < end) requestAnimationFrame(step);
    else done({ frames: frames.slice(1), places: places.size, fewest });
  };
  requestAnimationFrame(step);
}), ms);

/** A run of frame times as its median, its 95th and how many. */
function summary(frames) {
  const sorted = [...frames].sort((a, b) => a - b);
  return { median_ms: +sorted[sorted.length >> 1].toFixed(1), p95_ms: +sorted[Math.floor(sorted.length * 0.95)].toFixed(1), frames: sorted.length };
}

module.exports = { BARS, DARK_OPEN, GPU, MP4, STEP_MS, boxLog, boxOf, calcifer, carry, closeBox, openBox, openCanvas, drop, lastLook, launch, lookAt, noStall, panning, png, pointAt, renderOf, settled, signIn, summary, until, watchBoxes, wav };
