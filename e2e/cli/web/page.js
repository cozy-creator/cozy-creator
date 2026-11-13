// What the web proofs share: real media made here, a timeout on every wait, and a drop as the browser delivers one.
const { execFileSync } = require('child_process');
const { readFileSync, rmSync, writeFileSync, readdirSync, readlinkSync, realpathSync } = require('fs');
const { join } = require('path');
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
/** A camera's JPEG as FFmpeg meets it: 96×64, its end past the first 2 KiB read, behind 4 KiB of comment as a photo's EXIF is. FFmpeg's probe
 *  took such a file for an image2 sequence by its name, which Calcifer refuses, and every photo had no thumbnail (cl-888). */
function photo() {
  const small = Buffer.from('/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYwLjMxLjEwMgD/2wBDAAgQEBMQExYWFhYWFhoYGhsbGxoaGhobGxsdHR0iIiIdHR0bGx0dICAiIiUmJSMjIiMmJigoKDAwLi44ODpFRVP/xABaAAEBAQEBAQEAAAAAAAAAAAAEBQADAQcCAQEBAQEBAQEBAAAAAAAAAAAEAwUAAQIHBhABAQAAAAAAAAAAAAAAAAAAAQARAQEBAAAAAAAAAAAAAAAAAAEAEf/AABEIAEAAYAMBEgACEgADEgD/2gAMAwEAAhEDEQA/ABDGGaklIY0RqAxxs5JKSxpDUBjjZqSUljRGoDHGzkkpLGiNQGMNmpKSWNEaiMYbNSSkwaI1EYw2cklJY0RqIxhs1JSSxojUBjjZySEl7SGVfgYl9pXvkb92vi6+rrWuuuuvlwx79XSRfxA0p4xxs9JKShojUBjjZqSkljRGoDGGzUkpLGkNRGMNnJJSWNEaiMYbNSSksaI1AY42cklJY0RqAxxs5JKSxpDUBjjZqSUljRGoDGGzkkpLGiNRGMNnJJSWNIZlzGFVSRfA3ym1+t3X8Lde3l5e3t5dxuNBK1UaU4YwwEkpKGiNRGMNnJJSWNEaiMYbOSSksaQ1AY42aklJY0RqAxxs5JKSxojUBjDZySUljRGojGGzUkpMGiNRGMNnJJSWNEaiMYbOSSksaI3zq1+k3X8jda11111rXXXXXt5eXt112G40UrVBpzRjjBSSkkaI1AY42cklJY0RqAxhs1JKSxpDURjDZySUljRGojGGzkkpLGiNQGONmpJSWNEagMcbOSSkwaI3/9k=', 'base64');
  const note = Buffer.alloc(4100, 0x20);
  note.writeUInt16BE(0xfffe, 0);
  note.writeUInt16BE(note.length - 2, 2);
  return Buffer.concat([small.subarray(0, 2), note, small.subarray(2)]);
}

/** A PNG: a gradient over `shade`, or what `pixel(x, y)` says ([r, g, b]) at each point. */
function png(shade, width = 320, height = 200, alpha = false, pixel) {
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
      const [r, g, b] = pixel ? pixel(x, y) : [shade, (x * 255 / width) | 0, (y * 255 / height) | 0];
      rows[at++] = r;
      rows[at++] = g;
      rows[at++] = b;
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

/** After its pages close, let Calcifer release the generated project's reader and SQLite handles. */
async function removeGeneratedProject(folder, prefix) {
  if (!folder) return;
  if (!require('path').basename(folder).startsWith(prefix)) throw new Error(`refusing to remove a folder not generated by ${prefix}: ${folder}`);
  let blocked;
  try {
    await until(`${folder} released for cleanup`, () => {
      try {
        rmSync(folder, { recursive: true, force: true });
        return true;
      } catch (error) {
        if (error.code !== 'EPERM' && error.code !== 'EBUSY') {
          blocked = undefined;
          throw error;
        }
        blocked = error;
        return false;
      }
    }, Boolean);
  } catch (error) {
    throw blocked ?? error;
  }
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

/** The actual fit action, independent of what control last had keyboard focus. */
async function fitCanvas(page) {
  if (!(await page.getByRole('menu', { name: 'Zoom', exact: true }).count())) await page.getByRole('button', { name: /, more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await settled(page);
}

/** Whether this process holds the project's database; null where ownership cannot be read. */
function projectHeld(folder, pid) {
  if (!['linux', 'darwin'].includes(process.platform)) return null;
  const db = join(realpathSync(folder), 'project.db');
  if (process.platform === 'linux') {
    const fds = `/proc/${pid}/fd`;
    let unread;
    for (const fd of readdirSync(fds)) {
      try {
        if (readlinkSync(join(fds, fd)) === db) return true;
      } catch (error) {
        if (error.code !== 'ENOENT') unread ??= error;
      }
    }
    if (unread) throw unread;
    return false;
  }
  if (process.platform === 'darwin') {
    let said = '', unread;
    try {
      said = execFileSync('lsof', ['-p', String(pid), '-Fn'], { encoding: 'utf8', maxBuffer: 1 << 26 });
    } catch (error) {
      if (error.code === 'ENOENT') return null;
      unread = error;
      said = String(error.stdout ?? '');
    }
    if (said.split('\n').some((line) => line === `n${db}`)) return true;
    if (unread) throw unread;
    return false;
  }
  return null;
}

/** Observer provenance from delivered entries only: no extra geometry/layout reads and no error suppression. */
function observeResizes() {
  const Native = window.ResizeObserver;
  const evidence = { callbacks: [], errors: [] };
  window.resizeEvidence = evidence;
  let next = 0;
  window.ResizeObserver = class ResizeObserver extends Native {
    constructor(callback) {
      if (typeof callback !== 'function') { super(callback); return; }
      const id = ++next, producer = new Error().stack;
      super((entries, observer) => {
        evidence.callbacks.push({ id, producer, at_ms: performance.now(), count: entries.length,
          targets: entries.slice(0, 32).map(({ target, contentRect: r }) => ({
            at: `${target.tagName.toLowerCase()}${target.id ? `#${target.id}` : ''}.${String(target.getAttribute('class') ?? '').replaceAll(' ', '.')}`.slice(0, 200),
            asset: target.closest('.card')?.dataset.asset,
            rect: [r.x, r.y, r.width, r.height],
          })), truncated: entries.length > 32 });
        if (evidence.callbacks.length > 16) evidence.callbacks.shift();
        return Reflect.apply(callback, observer, [entries, observer]);
      });
    }
  };
  addEventListener('error', (event) => {
    if (!/ResizeObserver/.test(event.message)) return;
    evidence.errors.push({ message: event.message, source: event.filename?.split(/[?#]/)[0], line: event.lineno, column: event.colno,
      stack: event.error?.stack, at_ms: performance.now(), callbacks: evidence.callbacks.slice() });
    if (evidence.errors.length > 4) evidence.errors.shift();
  });
}

// The asset's current surface: Focus while open, otherwise its Wander node.
const boxOf = (page, name) => page.locator(`[data-focused-asset="${name}"], .react-flow__node:has(.card[data-asset="${name}"])`).first();
async function closeBox(page) {
  // By the address, as the page has it: after a reload the box is on its way, and not there to count yet, and `page.url()` hears of a move a moment late.
  // Left already, by a click off it say, the canvas may still be on its way back.
  if (!(await page.evaluate(() => location.pathname.includes('/assets/')))) return settled(page);
  await page.locator('.card.here').waitFor();
  await page.keyboard.press('Escape');
  await page.locator('.card.here').waitFor({ state: 'detached' });
  await settled(page);
}
// A double click works on a box where it stands: the middle of it, where there's nothing else to press.
/** An open box's Render. Image tools enter their explicit Edit mode before rendering. */
async function renderOf(page, name) {
  const here = page.locator(`.card.here[data-asset="${name}"]`).first();
  await here.waitFor();
  const edit = here.getByRole('button', { name: 'Edit image', exact: true });
  if (await edit.count()) await edit.click();
  const nextVideo = here.getByRole('button', { name: 'New video', exact: true });
  if (await nextVideo.count() && !(await here.locator('.in-frame').count())) await nextVideo.click();
  const next = here.locator('.take.add[aria-label="Its next take"]');
  // A cold tool list can briefly show fallback fields before an image take gains its +.
  // Wait for an editor (including non-image/custom editors) or that actual next-take control.
  const editor = here.locator('.in-frame, .prominent, .tl, .tool-ui, .tool-ui-failed, .media.blank:not(:has(.media-making, .made-from, .prog)), .media:has(.play, video, audio)');
  const render = here.locator('button[aria-label=Render]');
  // A box being written (a failed or unmade one's words) has its Render out already.
  await editor.or(next).or(nextVideo).or(render).first().waitFor();
  if (!(await render.count())) {
    if (await nextVideo.count() && !(await here.locator('.prominent').count())) await nextVideo.click();
    if (!(await here.locator('.in-frame').count()) && (await next.count())) await next.click();
  }
  await render.waitFor();
  return render;
}

async function openBox(page, name) {
  // By the address too: a box left a moment ago can still say so, and reaching for it finds it gone (macOS, 41977214625).
  // Open, it's in Focus; on a page loaded at its address its canvas card says `here` too, and Focus is the later.
  const card = page.locator(`.card.here[data-asset="${name}"]`);
  if (await page.evaluate((asked) => location.pathname.endsWith(`/assets/${encodeURIComponent(asked)}`), name)) return card.last().waitFor();
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
    await settled(page);
    await until(`${name} in reach`, reached, Boolean);
  }
  const { x, y } = await middle();
  const before = await page.evaluate(() => document.querySelector('.react-flow__viewport')?.style.transform);
  // A click only selects it: a double click opens it (Paul, 11-12).
  await page.mouse.dblclick(x, y);
  await card.last().waitFor();
  // The canvas draws in on it: a person reaches for it once it's there. Its move starts a frame after entry, which a slow runner draws late.
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
  for (const type of ['mousedown', 'mouseup', 'click', 'pointercancel', 'lostpointercapture', 'dragstart']) {
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

/** The file at `path` held in `project`'s own content store, sent as the page sends a file (`PUT /v1/project-files`): its `sha256:`. */
async function upload(token, path, project) {
  if (!project) throw new Error(`upload ${path}: no project to hold it`);
  const bytes = readFileSync(path);
  const digest = `sha256:${require('crypto').createHash('sha256').update(bytes).digest('hex')}`;
  const at = `/v1/project-files/${digest}?project=${encodeURIComponent(project)}`;
  const answer = await fetch(`${calcifer()}${at}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' }, body: bytes });
  if (!answer.ok) throw new Error(`PUT ${at}: ${answer.status} ${await answer.text()}`);
  const held = await answer.json();
  if (held.digest !== digest || held.length !== bytes.length) throw new Error(`PUT ${at}: the reply names another file or length`);
  return digest;
}

/** Sign this tab in with a fresh session code, opening `to` (a URL on this Calcifer), else its home. The session is the tab's (sessionStorage):
 *  a reload or another page in the tab keeps it, and another tab or context signs in again. The code lives 15 minutes. */
async function signIn(page, to, options) {
  if (typeof to === 'object') [to, options] = [undefined, to];
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const response = await fetch(`http://${holder.address}/v1/session-codes`, {
    method: 'POST', headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' }, body: '{}',
  });
  if (!response.ok) throw new Error(`session code: ${response.status} ${await response.text()}`);
  const code = await response.json();
  const link = to ? `${to.replace(/#.*$/, '')}#code=${encodeURIComponent(code.code)}`
    : process.env.PAGE_ORIGIN ? code.url.replace(/^http:\/\/[^/]+/, process.env.PAGE_ORIGIN) : code.url;
  const redeemed = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/session', { timeout: STEP_MS });
  const went = await page.goto(link, options);
  const answer = await redeemed;
  if (!answer.ok()) throw new Error(`sign in: POST /v1/session ${answer.status()}`);
  return went;
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

/** A native CLI command in the project directory, matching a person's shell. */
function cozyIn(folder, words, options = {}) {
  return execFileSync(process.env.COZY, words, { ...options, cwd: folder });
}

/** A separate process lets synchronous fixture readers talk to the real Calcifer. */
const HTTP_READ = `
const fs = require('node:fs');
const http = require('node:http');
const holder = JSON.parse(fs.readFileSync(process.env.HOLDER, 'utf8'));
const body = fs.readFileSync(0);
const url = new URL(process.argv[2], 'http://' + holder.address);
const request = http.request(url, { method: process.argv[1], headers: {
  Authorization: 'Bearer ' + holder.token,
  'cozy-project-storage': '3',
  'Content-Type': process.argv[3],
  'Content-Length': body.length,
}}, (response) => {
  const chunks = [];
  response.on('data', (chunk) => chunks.push(chunk));
  response.on('end', () => {
    const text = Buffer.concat(chunks);
    if (response.statusCode >= 400) { process.stderr.write(text); process.exitCode = 1; }
    else process.stdout.write(text);
  });
});
request.on('error', (error) => { console.error(error.message); process.exitCode = 1; });
request.end(body);
`;

function apiText(method, path, body = '', type = 'application/json') {
  return execFileSync(process.execPath, ['-e', HTTP_READ, method, path, type], {
    input: body, encoding: 'utf8', maxBuffer: 1 << 28, timeout: STEP_MS,
  });
}

const projectPath = (folder, suffix = '') => `/v1/projects/${encodeURIComponent(folder)}${suffix}`;
const projectAddresses = new Map();
async function projectRecord(reference) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const response = await fetch(`http://${holder.address}${projectPath(reference)}?assets=false`, { headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3' } });
  if (!response.ok) throw new Error(`project ${reference}: ${response.status} ${await response.text()}`);
  const project = await response.json();
  if (!/^[0-9A-HJKMNP-TV-Z]{9}$/.test(project.code ?? '')) throw new Error(`project ${project.id} has no canonical compact code`);
  projectAddresses.set(project.id, project);
  projectAddresses.set(project.code, project);
  return project;
}

/** The compact path of this exact canonical project, read from the API when needed. */
async function projectUrl(reference, suffix = '') {
  const project = typeof reference === 'string'
    ? projectAddresses.get(reference) ?? await projectRecord(reference)
    : reference;
  if (!/^[0-9A-HJKMNP-TV-Z]{9}$/.test(project.code ?? '')) throw new Error(`project ${project.id} has no canonical compact code`);
  return `${calcifer()}/p/${project.code}${suffix}`;
}

/** A browser URL names a code; API requests and test identities keep its canonical id. */
async function projectFromPage(page) {
  const code = new URL(page.url()).pathname.match(/^\/p\/([0-9A-HJKMNP-TV-Z]{9})(?:\/|$)/)?.[1];
  if (!code) throw new Error(`not a compact project URL: ${page.url()}`);
  const project = await projectRecord(code);
  if (project.code !== code) throw new Error(`the URL code ${code} resolved to ${project.code}`);
  return project;
}

function projectJson(folder, method = 'GET', suffix = '', body) {
  const text = apiText(method, projectPath(folder, suffix), body === undefined ? '' : JSON.stringify(body));
  return text ? JSON.parse(text) : null;
}

/** Where `folder`'s rows differ from the document they were written with (cl-885): a read takes the rows, so nowhere, after any write. */
function integrity(folder) {
  return projectJson(folder, 'GET', '/integrity')?.differences ?? ['no answer'];
}

/** Import a real file by the same content endpoint the browser uses. */
function importFile(folder, file, name) {
  const bytes = readFileSync(file);
  const sha256 = `sha256:${require('crypto').createHash('sha256').update(bytes).digest('hex')}`;
  apiText('PUT', `/v1/project-files/${sha256}?project=${encodeURIComponent(folder)}`, bytes, 'application/octet-stream');
  const stem = require('path').parse(file).name;
  return projectJson(folder, 'POST', '/imports', { source: sha256, name: name ?? stem, exact: name !== undefined });
}

/** Concurrent fixture mutations go directly to the existing project HTTP API. */
async function projectAction(folder, action, ...args) {
  const started = Date.now();
  const path = projectPath(folder);
  const request = async (method, suffix = '', body) => {
    const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
    const response = await fetch(`http://${holder.address}${path}${suffix}`, {
      method, headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
      signal: AbortSignal.timeout(STEP_MS),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) throw new Error(`${method} ${suffix}: ${response.status} ${text}`);
    if (!text) return null;
    return JSON.parse(text);
  };
  const fieldsOf = (terms) => Object.fromEntries(terms.map((term) => {
    const at = term.indexOf('=');
    if (at < 1) throw new Error(`fixture field has no value: ${term}`);
    let key = term.slice(0, at), value = term.slice(at + 1);
    if (key.endsWith(':')) { key = key.slice(0, -1); value = JSON.parse(value); }
    else if (!value) value = null;
    else if (/^(true|false|-?\d+(\.\d+)?)$/.test(value)) value = JSON.parse(value);
    return [key, value];
  }));
  const takePlace = (fields) => {
    if (!Object.hasOwn(fields, 'at')) return undefined;
    const [x, y, w] = String(fields.at).split(',').map(Number);
    delete fields.at;
    return w === undefined ? { x, y } : { x, y, w };
  };
  let value;
  switch (action) {
    case 'status': value = await request('GET', args[0] ? `/assets/${encodeURIComponent(args[0])}` : ''); break;
    case 'add': {
      const [tool, name, ...terms] = args;
      const fields = fieldsOf(terms);
      const at = takePlace(fields);
      value = await request('POST', '/assets', { tool: tool === 'text' ? '' : tool, name, exact: true, fields, at });
      break;
    }
    case 'set': {
      const [name, ...terms] = args;
      const fields = terms[0] === '--input' ? JSON.parse(readFileSync(terms[1], 'utf8')) : fieldsOf(terms);
      const at = takePlace(fields);
      const tool = fields.tool;
      delete fields.tool;
      if (Object.keys(fields).length || tool !== undefined) value = await request('PATCH', `/assets/${encodeURIComponent(name)}`, { fields, tool });
      if (at) value = await request('PATCH', '/canvas', { cards: { [name]: at } });
      break;
    }
    case 'import': {
      const [source, name, , description] = args;
      if (/^(sha256:)?[0-9a-f]{6,64}$/i.test(source)) value = await request('POST', '/imports', { source, name, description, exact: true });
      else {
        const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
        const sourceHash = await upload(holder.token, source, folder);
        value = await request('POST', '/imports', { source: sourceHash, name: name ?? require('path').parse(source).name, exact: name !== undefined });
      }
      break;
    }
    case 'delete': {
      for (const name of args) value = await request('DELETE', `/assets/${encodeURIComponent(name)}`);
      const history = await request('GET', '/history');
      value.history = history.entries.find((entry) => entry.words?.startsWith('Deleted ') && entry.names?.some((name) => args.includes(name)))?.id;
      break;
    }
    case 'approve': value = await request('POST', '/approve', { name: args[0], version: args[1] }); break;
    case 'unapprove': value = await request('POST', '/unpin', { name: args[0] }); break;
    case 'rename': value = await request('POST', '/rename', { from: args[0], to: args[1] }); break;
    case 'cancel': value = await request('POST', '/cancel', { names: args }); break;
    case 'history': value = await request('GET', `/history?limit=${args[1] ?? 50}`); break;
    case 'undo':
    case 'redo': value = await request('POST', `/history/${action}`, args[0] ? { entry: Number(args[0]) } : {}); break;
    case 'render': {
      const index = args.indexOf('--on');
      const on = index < 0 ? [] : args[index + 1].split(',');
      const firstFlag = args.findIndex((arg) => arg.startsWith('--'));
      const names = firstFlag < 0 ? args : args.slice(0, firstFlag);
      value = await request('POST', '/runs', { names, on });
      if (!args.includes('--queue')) {
        const state = await until('fixture render ends', () => request('GET'), (state) => !state.busy, 300_000);
        const failed = state.board?.jobs?.filter((job) => ['failed', 'not_sent'].includes(job.state));
        if (failed?.length) throw new Error(`fixture render failed: ${JSON.stringify(failed)}`);
      }
      break;
    }
    default: throw new Error(`unknown fixture action ${action}`);
  }
  const at = Date.now();
  return { value, at, ms: at - started };
}

/** Create starter-film fixtures through the project API. */
async function newProject(folder) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const answer = await fetch(`http://${holder.address}/v1/projects`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
    body: JSON.stringify({ path: folder, starter: true, unencrypted: true }),
  });
  const opened = await answer.json();
  if (!answer.ok) throw new Error(`create starter project ${folder}: ${answer.status} ${JSON.stringify(opened)}`);
  return opened;
}

/** `boxes` made in `folder`'s project in order, each a `POST /assets` body ({name, tool, kind, fields, at}) exactly named, as the page makes
 * them. An empty tool needs `fields.text` or a `kind`. */
function addBoxes(folder, boxes) {
  return boxes.map((box) => projectJson(folder, 'POST', '/assets', { tool: '', exact: true, ...box }));
}

/** Each box of `folder`'s project as `GET /v1/projects/{id}` answers it: name, tool, flat fields, approved, versions, … */
function assetsOf(folder) {
  return projectJson(folder).assets;
}

/** Each box's fields as project.db stores them, raw (an `approved` hash prefix, `tracks`), read only: {name: {field: value}}, in order. */
function storedOf(folder) {
  const stored = {};
  for (const [name, field, value] of rowsOf(folder, 'SELECT b.name, f.field, f.value FROM boxes b LEFT JOIN fields f ON f.box = b.key ORDER BY b.ord, f.ord')) {
    stored[name] ??= {};
    if (field !== null) stored[name][field] = JSON.parse(value);
  }
  return stored;
}

/** Rows of `folder`'s project.db for `query`, read only, each an array. */
function rowsOf(folder, query) {
  const script = 'import json, sqlite3, sys\nfor row in sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True).execute(sys.argv[2]): print(json.dumps(list(row)))';
  return execFileSync(PYTHON, ['-c', script, `${folder}/project.db`, query], { encoding: 'utf8' }).split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

/** Expand a tool category without creating anything and return one actual draggable implementation. */
async function implementationRow(page, operation, id) {
  const focused = page.locator('[data-focused-asset]');
  if (await focused.count()) {
    await focused.getByRole('button', { name: 'Back to canvas', exact: true }).click();
    await focused.waitFor({ state: 'detached' });
  }
  const show = page.locator('aside.tools [aria-label="Show the tools"]');
  if (await show.count()) await show.click();
  const category = page.locator(`.tools [data-operation="${operation}"]`);
  await category.waitFor();
  if (await category.getAttribute('aria-expanded') !== 'true') await category.click();
  const rows = page.locator(`.tools [data-implementation][data-tool-operation="${operation}"]`);
  const row = id ? page.locator(`.tools [data-implementation][data-tool-operation="${operation}"][data-tool=${JSON.stringify(id)}]`) : rows.first();
  await row.waitFor();
  return row;
}

/** A thumbnail click previews a retained state without approving it. */
async function previewVersion(page, name, wanted) {
  await openBox(page, name);
  const card = boxOf(page, name);
  const take = card.locator(`.image-history.here-side .take[data-take="${wanted}"]`);
  await take.waitFor();
  await take.scrollIntoViewIfNeeded();
  await take.click();
  await until(`${name} thumbnail is previewed`, () => card.locator('.image-history.here-side .take.on[data-take]').getAttribute('data-take'), (number) => number === String(wanted));
}

/** Approval is explicit and separate from thumbnail preview. */
async function approveVersion(page, name, wanted) {
  await previewVersion(page, name, wanted);
  const folder = (await projectFromPage(page)).path;
  const item = projectJson(folder).assets.find((asset) => asset.name === name);
  if (!item.pinned || item.approved !== wanted) await boxOf(page, name).locator(`.image-history.here-side .take[data-take="${wanted}"]`).dblclick();
  await until(`${name} version is approved`, () => Promise.resolve(projectJson(folder).assets.find((asset) => asset.name === name)), (asset) => asset?.version === wanted && asset?.approved === wanted && asset?.pinned);
}

/** Replace the tool with a real compatible implementation dragged from its category. */
async function chooseModel(page, name, id, operation) {
  const folder = (await projectFromPage(page)).path;
  const card = boxOf(page, name);
  const tool = await implementationRow(page, operation, id);
  const target = await card.locator('.media').boundingBox();
  await carry(page, tool, { x: target.x + target.width / 2, y: target.y + target.height / 2 });
  await until(`tool ${id} attached`, () => Promise.resolve(projectJson(folder).assets.find((asset) => asset.name === name)?.tool), (held) => held === id);
  await settled(page);
  if (await page.locator('[data-focused-asset]').count()) throw new Error('Attaching a tool entered Focus before an explicit double-click');
  await openBox(page, name);
}

/** Python as this OS names it: Windows has no python3 of its own. */
const PYTHON = process.platform === 'win32' ? 'python' : 'python3';

module.exports = { addBoxes, assetsOf, storedOf, integrity, implementationRow, previewVersion, approveVersion, chooseModel, projectUrl, projectFromPage, projectJson, importFile, projectAction, cozyIn, newProject, BARS, DARK_OPEN, GPU, MP4, PYTHON, STEP_MS, boxLog, boxOf, calcifer, carry, closeBox, openBox, openCanvas, drop, lastLook, launch, lookAt, noStall, panning, png, pointAt, removeGeneratedProject, renderOf, rowsOf, settled, fitCanvas, projectHeld, observeResizes, photo, signIn, summary, until, upload, watchBoxes, wav };
