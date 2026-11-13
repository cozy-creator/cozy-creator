// A project opens at once (cl-982; Paul, 11-13: "all of the nodes should appear immediately ... the previews should all appear very quickly;
// after all, all files are already on disc"). Two copies of one project of 50 real pictures of a few MB each, one plaintext and one
// encrypted, opened in a real Chrome on a real Calcifer: when each box's frame is drawn and when each preview is, from the project's first
// state; how long a request waited for a connection, and the most connections to Calcifer at once; with one tab and with two more of the
// same Calcifer open. A box whose preview is on its way pulses, faintly (screenshots in light and dark, 1600 wide). Its thumbnails are
// stored in the project as its files land: an open makes none, and one for a file added while it's open is drawn soon after it lands.
// Zoomed out from 100%, every box in the new view is drawn in each frame of the zoom.
//   node opens.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $ALONE: on a shared
// computer, so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { join } = require('path');
const zlib = require('zlib');
const { calcifer, lastLook, lookAt, noStall, projectAction, projectJson, projectUrl, signIn, STEP_MS, upload } = require('./page');
const { runnerSpeed } = require('./speed');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}

const PICTURES = 50;
const SIDE = 1200;
const COLUMNS = 10;
const [W, DX, DY] = [240, 280, 320];
/** Long enough for the old build's stall to show whole, short enough that the cell ends. */
const WAIT_MS = 75_000;
const VIEWPORT = { width: 1600, height: 1000 };
// What Paul asked (11-13), on the fast hosted runners; each is held times this runner's speed against those (speed.js).
const BARS = { frames_first_ms: 100, frames_all_ms: 100, previews_cold_ms: 2000, previews_warm_ms: 1000, landed_ms: 2000 };
const SPEED = runnerSpeed();
const BAR = Object.fromEntries(Object.entries(BARS).map(([name, ms]) => [name, Math.round(ms * SPEED.factor)]));
seen.runner = SPEED;
seen.bars = BAR;

/** A real picture of a few MB: a gradient under noise, `i` telling one from another, as a PNG. */
function picture(i) {
  const row = SIDE * 3 + 1;
  const rows = Buffer.alloc(row * SIDE);
  let s = (i + 1) * 2654435761 >>> 0 || 1;
  for (let y = 0; y < SIDE; y++) {
    let at = y * row;
    rows[at++] = 0;
    for (let x = 0; x < SIDE; x++) {
      s ^= s << 13; s ^= s >>> 17; s ^= s << 5;
      const n = (s >>> 26) & 31;
      rows[at++] = (i * 37 + (x >> 2) + n) & 255;
      rows[at++] = ((y >> 2) + n) & 255;
      rows[at++] = (i * 11 + ((x + y) >> 3) + n) & 255;
    }
  }
  const table = new Int32Array(256).map((_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c; });
  const crc = (bytes) => { let c = -1; for (const b of bytes) c = table[(c ^ b) & 255] ^ (c >>> 8); return (c ^ -1) >>> 0; };
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const head = Buffer.alloc(4), sum = Buffer.alloc(4);
    head.writeUInt32BE(data.length);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([head, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(SIDE, 0);
  header.writeUInt32BE(SIDE, 4);
  header[8] = 8;
  header[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', zlib.deflateSync(rows, { level: 1 })), chunk('IEND', Buffer.alloc(0))]);
}

// A grid of 51 places, one left empty inside it for a picture landing while it's open.
const HOLE = 45;
const slotOf = (i) => (i === PICTURES ? HOLE : i < HOLE ? i : i + 1);
const placeOf = (i) => ({ x: (slotOf(i) % COLUMNS) * DX, y: Math.floor(slotOf(i) / COLUMNS) * DY, w: W });
const named = (i) => `pic_${i}`;
// The whole grid in view.
const FIT = { x: 70, y: 90, zoom: 0.45 };

/** Each frame, from the page's start: when each box is first drawn, when its preview is, and whether it pulses while it's on its way. */
const SAMPLER = () => {
  try { performance.setResourceTimingBufferSize(10_000); } catch {}
  const seen = { nodes: {}, frames: 0, misses: [], long: [] };
  window.__opens = seen;
  // What held the page's thread: what a first frame waits behind.
  try {
    new PerformanceObserver((list) => {
      for (const one of list.getEntries()) {
        const at = (ms) => Math.round(ms - one.startTime);
        const scripts = [...(one.scripts ?? [])].sort((a, b) => b.duration - a.duration).slice(0, 4)
          .map((script) => [script.invoker, Math.round(script.duration), Math.round(script.forcedStyleAndLayoutDuration), script.sourceFunctionName, script.sourceCharPosition]);
        seen.long.push({ at: Math.round(one.startTime), ms: Math.round(one.duration), render: at(one.renderStart), style: at(one.styleAndLayoutStart), scripts });
      }
    }).observe({ type: 'long-animation-frame', buffered: true });
  } catch {}
  const pulsing = (node) => node.getAnimations({ subtree: true }).some((one) => one.animationName === 'loading-pulse');
  // Light, as it runs in every frame of every tab on the page's own thread: a box is read in full only until its picture is drawn, its
  // place read once a change, and whether it pulses every fourth frame. React Flow hides a box it hasn't measured by its inline style.
  const onScreen = (node) => {
    const box = node.getBoundingClientRect();
    return box.width > 0 && box.height > 0 && box.right > 0 && box.bottom > 0 && box.left < innerWidth && box.top < innerHeight;
  };
  const tick = () => {
    const now = performance.now();
    seen.frames++;
    for (const node of document.querySelectorAll('.react-flow__node')) {
      const name = node.querySelector('.card[data-asset]')?.dataset.asset;
      if (!name) continue;
      const row = seen.nodes[name] ??= { frame: null, preview: null, waiting: 0, pulsing: 0, onScreen: false };
      if (row.preview !== null || node.style.visibility === 'hidden') continue;
      if (row.frame === null) {
        row.frame = now;
        row.onScreen = onScreen(node);
      }
      const host = node.querySelector('.buffered-image');
      const img = host?.querySelector('img');
      if (host?.dataset.displayedSha && img?.complete && img.naturalWidth > 0) {
        row.preview = now;
        row.onScreen = onScreen(node);
        continue;
      }
      // One on screen without its picture yet pulses, once it's been up three frames (50 ms).
      if (seen.frames % 4 || now - row.frame <= 50) continue;
      row.onScreen = onScreen(node);
      if (!row.onScreen) continue;
      row.waiting++;
      if (pulsing(node)) row.pulsing++;
      else if (seen.misses.length < 20) seen.misses.push({ name, at: Math.round(now) });
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
};

/** Every box's place on screen, each frame for `ms`, and the canvas's transform then. */
const FRAMES = (ms) => new Promise((done) => {
  const frames = [];
  const end = performance.now() + ms;
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const tick = () => {
    const viewport = document.querySelector('.react-flow__viewport');
    const drawn = {};
    for (const node of document.querySelectorAll('.react-flow__node')) {
      const name = node.querySelector('.card[data-asset]')?.dataset.asset;
      const box = node.getBoundingClientRect();
      if (name && box.width > 0 && box.height > 0 && node.style.visibility !== 'hidden') drawn[name] = [box.x, box.y, box.width, box.height];
    }
    frames.push({ at: performance.now(), transform: viewport ? getComputedStyle(viewport).transform : null, drawn });
    if (performance.now() < end) requestAnimationFrame(tick);
    else done({ pane: [pane.x, pane.y, pane.width, pane.height], frames });
  };
  requestAnimationFrame(tick);
});

/** Each file a thumbnail is kept as under `dir`, by name: what's made while a project opens. */
function thumbnails(dirs) {
  const files = new Set();
  const walk = (dir) => {
    let entries;
    try { entries = readdirSync(dir, { withFileTypes: true }); } catch { return; }
    for (const entry of entries) {
      const at = join(dir, entry.name);
      if (entry.isDirectory()) walk(at);
      else if (!entry.name.startsWith('.making')) files.add(at);
    }
  };
  for (const dir of dirs) walk(dir);
  return files;
}

/** Requests to Calcifer from every tab of a browser, as Chrome's network log has them: when each was asked, sent, and ended, on which
 *  connection. A stream that never ends holds its connection to the last. */
class Wire {
  constructor(base) {
    this.base = base;
    this.requests = new Map();
    this.now = 0;
  }
  async watch(page, tab) {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Network.enable');
    // A document replaced ends what it held, which Chrome's log doesn't always say of a stream.
    page.on('framenavigated', (frame) => {
      if (frame !== page.mainFrame()) return;
      for (const row of this.requests.values()) if (row.tab === tab && row.ended === null && row.asked < this.now) row.ended = this.now;
    });
    const at = (event) => { this.now = Math.max(this.now, event.timestamp ?? 0); };
    cdp.on('Network.requestWillBeSent', (event) => {
      at(event);
      if (!event.request.url.startsWith(this.base)) return;
      const url = new URL(event.request.url);
      this.requests.set(`${tab}:${event.requestId}`, { tab, path: url.pathname, query: url.search, asked: event.timestamp, sent: null, ended: null, connection: null, status: null, cached: false });
    });
    cdp.on('Network.responseReceived', (event) => {
      at(event);
      const row = this.requests.get(`${tab}:${event.requestId}`);
      if (!row) return;
      const { response } = event;
      row.status = response.status;
      row.connection = response.connectionId || null;
      row.cached = Boolean(response.fromDiskCache || response.fromMemoryCache || response.fromServiceWorker);
      if (response.timing) {
        row.sent = response.timing.requestTime + response.timing.sendStart / 1000;
        row.answered = response.timing.requestTime + response.timing.receiveHeadersEnd / 1000;
      }
    });
    for (const ended of ['Network.loadingFinished', 'Network.loadingFailed']) {
      cdp.on(ended, (event) => {
        at(event);
        const row = this.requests.get(`${tab}:${event.requestId}`);
        if (row) row.ended = event.timestamp;
      });
    }
    cdp.on('Network.requestServedFromCache', (event) => {
      const row = this.requests.get(`${tab}:${event.requestId}`);
      if (row) row.cached = true;
    });
  }
  /** What was asked from `from` (a network-log time) on: the most connections in use at once, and how long thumbnails waited for one. */
  since(from) {
    const rows = [...this.requests.values()];
    const open = rows.filter((row) => row.connection && row.sent !== null && (row.ended ?? Infinity) >= from);
    const moments = open.flatMap((row) => [[Math.max(row.sent, from), 1, row.connection], [row.ended ?? Infinity, -1, row.connection]]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const using = new Map();
    let most = 0;
    for (const [, step, connection] of moments) {
      using.set(connection, (using.get(connection) ?? 0) + step);
      if (!using.get(connection)) using.delete(connection);
      most = Math.max(most, using.size);
    }
    const thumbs = rows.filter((row) => row.asked >= from && /\/thumbs\//.test(row.path));
    // A picture from the browser's cache says when it was first sent, before it was asked again.
    const waited = thumbs.filter((row) => row.sent !== null && !row.cached && row.sent >= row.asked).map((row) => (row.sent - row.asked) * 1000).sort((a, b) => a - b);
    const statuses = {};
    for (const row of thumbs) statuses[row.status ?? 'none'] = (statuses[row.status ?? 'none'] ?? 0) + 1;
    // How long Calcifer took to answer each (ffmpeg's time, when a request made one), and how long the page's other calls waited for a
    // connection behind them.
    const answered = thumbs.filter((row) => row.sent !== null && row.answered && !row.cached && row.sent >= row.asked).map((row) => (row.answered - row.sent) * 1000).sort((a, b) => a - b);
    const calls = rows.filter((row) => row.asked >= from && !/\/thumbs\/|\/events$/.test(row.path) && row.sent !== null && row.sent >= row.asked).map((row) => (row.sent - row.asked) * 1000).sort((a, b) => a - b);
    const held = rows.filter((row) => row.connection && row.sent !== null && row.ended === null).map((row) => `${row.tab} ${row.path}`);
    return {
      most_connections: most,
      held_open: held,
      thumbs: thumbs.length,
      thumb_statuses: statuses,
      thumbs_cached: thumbs.filter((row) => row.cached).length,
      thumb_wait_ms: waited.length ? { median: Math.round(waited[waited.length >> 1]), most: Math.round(waited.at(-1)) } : null,
      thumb_answer_ms: answered.length ? { median: Math.round(answered[answered.length >> 1]), most: Math.round(answered.at(-1)) } : null,
      call_wait_ms: calls.length ? { median: Math.round(calls[calls.length >> 1]), most: Math.round(calls.at(-1)), calls: calls.length } : null,
    };
  }
}

/** The file at `path` in `project`, sent as the page sends one (`PUT /v1/project-files`), then imported. */
async function importInto(project, path, name) {
  const { token } = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const digest = await upload(token, path, project.id);
  return (await projectAction(project.folder, 'import', digest, name)).value;
}

const ms = (value) => (value === null || value === undefined ? null : Math.round(value));
const quantile = (values, q) => (values.length ? values[Math.min(values.length - 1, Math.floor(q * values.length))] : null);

(async () => {
  if (process.env.ALONE) {
    seen.ran = false;
    console.log(JSON.stringify({ failures, seen }, null, 2));
    return;
  }
  const base = calcifer();
  const scratch = mkdtempSync(join(tmpdir(), 'web-opens-'));
  seen.project = { folder: scratch };
  const plain = join(scratch, 'plain');
  const sealed = join(scratch, 'sealed');
  const files = join(scratch, 'pictures');
  execFileSync('mkdir', ['-p', files]);
  const cozy = (...words) => execFileSync(process.env.COZY, words, { encoding: 'utf8', timeout: 180_000 });

  during = 'the fixture';
  const made = Date.now();
  const paths = [];
  for (let i = 0; i <= PICTURES; i++) {
    const at = join(files, `${named(i)}.png`);
    writeFileSync(at, picture(i));
    paths.push(at);
  }
  seen.fixture = { pictures: PICTURES, bytes: Math.round(statSync(paths[0]).size / 1e5) / 10 + ' MB each', made_ms: Date.now() - made };
  cozy('--json', 'new', '--unencrypted', plain);
  const secrets = join(scratch, 'secrets');
  execFileSync('mkdir', ['-p', secrets]);
  writeFileSync(join(secrets, 'pass'), 'synthetic opens passphrase\n');
  cozy('--json', 'new', sealed, '--passphrase-file', join(secrets, 'pass'), '--recovery-out', join(secrets, 'recovery'));
  const vault = JSON.parse(cozy('--json', 'vault', '--directory', sealed, 'status'));
  const view = vault.view ?? vault.vault?.view;
  check('the_sealed_copy_is_encrypted_and_unlocked', vault.encrypted === true && vault.state === 'unlocked' && Boolean(view), vault);
  const projects = {
    plain: { folder: plain, view: plain, sealed: false },
    sealed: { folder: sealed, view, sealed: true },
  };
  const importing = Date.now();
  for (const [kind, project] of Object.entries(projects)) {
    project.id = projectJson(project.folder, 'GET', '?assets=false').id;
    project.url = await projectUrl(project.id);
    const cards = {};
    for (let i = 0; i < PICTURES; i++) cards[(await importInto(project, paths[i], named(i))).name] = placeOf(i);
    projectJson(project.folder, 'PATCH', '/canvas', { cards });
    // Its view kept in it, as the page keeps one: the whole grid.
    projectJson(project.folder, 'PATCH', '/view', { viewport: FIT });
    seen.fixture[kind] = { id: project.id, imported_ms: Date.now() - importing };
  }
  // Where a thumbnail can be kept: the project's own store, and the caches before it.
  const stores = (project) => [join(project.view, '.thumbs'), join(homedir(), '.cozy', 'cache', 'thumbs'), join(project.view, '.vault-calcifer', 'thumbs')];

  during = 'thumbnails stored as the files landed';
  const storing = Date.now();
  let stored = {};
  while (Date.now() - storing < 60_000) {
    stored = Object.fromEntries(Object.entries(projects).map(([kind, project]) => [kind, thumbnails([join(project.view, '.thumbs')]).size]));
    if (Object.values(stored).every((count) => count >= PICTURES)) break;
    await new Promise((done) => setTimeout(done, 250));
  }
  seen.stored = { ...stored, ms: Date.now() - storing };
  check('every_landed_picture_has_its_thumbnail_stored_in_its_project', Object.values(stored).every((count) => count >= PICTURES), seen.stored);

  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });

  /** A browser of its own, signed in: its cache cold. */
  const fresh = async (theme = 'paper') => {
    const context = await browser.newContext({ viewport: VIEWPORT });
    context.setDefaultTimeout(STEP_MS);
    await context.addInitScript(SAMPLER);
    await context.addInitScript((theme) => {
      try { localStorage.setItem('cozy.theme', theme); } catch {}
    }, theme);
    const wire = new Wire(base);
    const errors = [];
    // Each tab opens the page itself.
    const tab = async (label) => {
      const page = await context.newPage();
      page.on('pageerror', (error) => errors.push(`${label}: ${error.message}`));
      await wire.watch(page, label);
      await signIn(page);
      await page.waitForSelector('.top');
      return page;
    };
    const first = await tab('a');
    lookAt(first);
    return { context, wire, errors, tab, first };
  };

  /** `page` opened on `project` (or reloaded): when its boxes and previews were drawn, against the bars of a `cold` or warm open. */
  const opening = async (label, page, wire, project, { cold, reload = false, others = [], go = true, bars = true } = {}) => {
    during = label;
    const before = go ? thumbnails(stores(project)) : new Set();
    const from = wire.now;
    if (reload) await page.reload();
    else if (go) await page.goto(project.url);
    const read = () => page.evaluate(({ id, pictures }) => {
      const opens = window.__opens;
      const path = (entry) => new URL(entry.name).pathname;
      const resources = performance.getEntriesByType('resource');
      const stateRead = resources.find((entry) => path(entry) === `/v1/projects/${encodeURIComponent(id)}` && !new URL(entry.name).search);
      const canvasRead = resources.find((entry) => path(entry) === `/v1/projects/${encodeURIComponent(id)}/canvas`);
      const rows = Object.entries(opens?.nodes ?? {}).filter(([name]) => /^pic_\d+$/.test(name) && Number(name.slice(4)) < pictures);
      const shown = rows.filter(([, row]) => row.onScreen);
      const api = resources.filter((entry) => path(entry).startsWith('/v1/') && !path(entry).includes('/thumbs/') && entry.startTime < 3000);
      return {
        state: stateRead && canvasRead ? Math.max(stateRead.responseEnd, canvasRead.responseEnd) : null,
        state_read_ms: stateRead ? Math.round(stateRead.responseEnd - stateRead.startTime) : null,
        frames: rows.map(([, row]) => row.frame).filter((at) => at !== null).sort((a, b) => a - b),
        previews: rows.map(([, row]) => row.preview).filter((at) => at !== null).sort((a, b) => a - b),
        on_screen: shown.length,
        on_screen_frames: shown.map(([, row]) => row.frame).filter((at) => at !== null).sort((a, b) => a - b),
        on_screen_previews: shown.map(([, row]) => row.preview).filter((at) => at !== null).sort((a, b) => a - b),
        trace: {
          reads: api.map((entry) => [`${path(entry).replace(/project-[0-9a-f]+/, 'P')}${new URL(entry.name).search}`, Math.round(entry.startTime), Math.round(entry.responseEnd)]).sort((a, b) => a[2] - b[2]).slice(0, 24),
          long: (opens?.long ?? []).filter(({ at }) => at < 3000),
        },
        waiting: rows.filter(([, row]) => row.waiting > 0).length,
        pulsing: rows.filter(([, row]) => row.waiting > 0 && row.pulsing === row.waiting).length,
        misses: opens?.misses ?? [],
      };
    }, { id: project.id, pictures: PICTURES });
    const began = Date.now();
    let now = await read();
    while ((now.previews.length < PICTURES && (now.on_screen === 0 || now.on_screen_previews.length < now.on_screen)) && Date.now() - began < WAIT_MS) {
      await new Promise((done) => setTimeout(done, 100));
      now = await read();
    }
    const after = thumbnails(stores(project));
    const made = [...after].filter((at) => !before.has(at)).length;
    const state = now.state;
    // From its first state, or, when that came during a long frame of no script of the page's (its first paint: ~150 ms of style and
    // layout on a runner that rasters in software), from that frame's end: nothing that came in can be handled sooner. Both are kept;
    // the bar is on this. A frame the project's own work made long (its scripts) is never taken off.
    const scripted = (frame) => frame.scripts.reduce((sum, [, lasts]) => sum + lasts, 0);
    const busy = state === null ? null : now.trace.long.find((frame) => frame.at <= state && state < frame.at + frame.ms && scripted(frame) < 20);
    const ready = state === null ? null : Math.max(state, busy ? busy.at + busy.ms : state);
    const sinceState = (at) => (ready === null || at === null || at === undefined ? null : Math.round(at - ready));
    const result = {
      tabs: 1 + others.length,
      state_ms: ms(state),
      state_read_ms: now.state_read_ms,
      first_frame_ms: sinceState(now.frames[0]),
      first_frame_from_state_ms: state === null || now.frames[0] === undefined ? null : Math.round(now.frames[0] - state),
      busy_when_state_came: busy ?? null,
      all_frames_ms: now.on_screen && now.on_screen_frames.length >= now.on_screen ? sinceState(now.on_screen_frames[now.on_screen - 1]) : null,
      frames: now.frames.length,
      first_preview_ms: ms(now.previews[0]),
      median_preview_ms: ms(quantile(now.previews, 0.5)),
      all_previews_ms: now.previews.length >= PICTURES ? ms(now.previews[PICTURES - 1]) : null,
      previews: now.previews.length,
      // What's on screen of it is what's held to the bar: all 50 in its kept view.
      on_screen: now.on_screen,
      on_screen_previews_ms: now.on_screen && now.on_screen_previews.length >= now.on_screen ? ms(now.on_screen_previews[now.on_screen - 1]) : null,
      preview_times_ms: now.previews.map(ms),
      trace: now.trace,
      waited_for_preview: now.waiting,
      pulsed_while_waiting: now.pulsing,
      unpulsed: now.misses.slice(0, 5),
      thumbnails_made: made,
      wire: wire.since(from),
    };
    seen[label] = result;
    if (!bars) {
      check(`${label}_every_preview_on_screen_drawn`, result.on_screen > 0 && result.on_screen_previews_ms !== null, { on_screen: result.on_screen, previews: result.previews, all: result.all_previews_ms });
      check(`${label}_every_box_waiting_for_its_preview_pulses`, result.pulsed_while_waiting === result.waited_for_preview, { waited: result.waited_for_preview, pulsed: result.pulsed_while_waiting, unpulsed: result.unpulsed });
      return result;
    }
    const bar = cold ? BAR.previews_cold_ms : BAR.previews_warm_ms;
    // Every frame on screen is drawn in one go: its first and its last. How soon is held over all the opens (below), as one open's frames
    // can wait out a frame of the page's other work on a runner that rasters in software.
    check(`${label}_every_frame_on_screen_drawn`, result.all_frames_ms !== null, { first: result.first_frame_ms, all: result.all_frames_ms, frames: result.frames, on_screen: result.on_screen });
    framed.push(result.all_frames_ms ?? Infinity);
    check(`${label}_every_preview_on_screen_within_${bar}ms`, result.on_screen > 0 && result.on_screen_previews_ms !== null && result.on_screen_previews_ms <= bar, { on_screen: result.on_screen, all: result.on_screen_previews_ms, previews: result.previews, median: result.median_preview_ms });
    check(`${label}_every_box_waiting_for_its_preview_pulses`, result.pulsed_while_waiting === result.waited_for_preview, { waited: result.waited_for_preview, pulsed: result.pulsed_while_waiting, unpulsed: result.unpulsed });
    if (cold) check(`${label}_makes_no_thumbnail`, made === 0, made);
    return result;
  };

  // When each open drew all of its frames on screen, after its state.
  const framed = [];
  for (const kind of ['plain', 'sealed']) {
    const project = projects[kind];
    const { context, wire, errors, first } = await fresh();
    await opening(`${kind}_cold`, first, wire, project, { cold: true });
    await opening(`${kind}_warm`, first, wire, project, { cold: false, reload: true });
    // All of it in view: each of the 50 read from its store (an encrypted one's from its encrypted store, never the browser's cache).
    during = `${kind}_fitted`;
    const fitting = await first.evaluate(() => performance.now());
    await first.keyboard.press('f');
    const fitted = await first.waitForFunction((n) => {
      const times = Object.entries(window.__opens?.nodes ?? {}).filter(([name]) => /^pic_\d+$/.test(name) && Number(name.slice(4)) < n).map(([, row]) => row.preview);
      return times.length >= n && times.every((at) => at !== null) ? Math.max(...times) : false;
    }, PICTURES, { timeout: WAIT_MS }).then((handle) => handle.jsonValue(), () => null);
    seen[`${kind}_fitted`] = { all_previews_ms: fitted === null ? null : Math.max(0, Math.round(fitted - fitting)) };
    check(`${kind}_fitted_all_${PICTURES}_previews_within_${BAR.previews_warm_ms}ms`, fitted !== null && fitted - fitting <= BAR.previews_warm_ms, seen[`${kind}_fitted`]);
    check(`${kind}_no_page_errors`, errors.length === 0, errors);
    await context.close();
  }

  // Two other tabs of this Calcifer open first, showing the other copy; then this one opens, cold, then again warm.
  for (const [kind, other] of [['plain', 'sealed'], ['sealed', 'plain']]) {
    const { context, wire, errors, tab, first } = await fresh();
    during = `${kind}_three_tabs: the others`;
    await first.goto(projects[other].url);
    const second = await tab('b');
    await second.goto(projects[other].url);
    for (const page of [first, second]) await page.waitForFunction((n) => Object.values(window.__opens?.nodes ?? {}).filter((row) => row.preview !== null).length >= n, PICTURES, { timeout: WAIT_MS }).catch(() => {});
    const third = await tab('c');
    await opening(`${kind}_three_tabs_cold`, third, wire, projects[kind], { cold: true, others: [first, second] });
    await opening(`${kind}_three_tabs_warm`, third, wire, projects[kind], { cold: false, reload: true, others: [first, second] });
    check(`${kind}_three_tabs_no_page_errors`, errors.length === 0, errors);
    await context.close();
  }

  // An old project's thumbnails, none stored: each made in the background as it's asked for, its box pulsing until it's drawn.
  for (const theme of ['paper', 'dark']) {
    const project = projects.plain;
    rmSync(join(project.view, '.thumbs'), { recursive: true, force: true });
    rmSync(join(homedir(), '.cozy', 'cache', 'thumbs'), { recursive: true, force: true });
    const { context, wire, first } = await fresh(theme);
    during = `pulse_${theme}`;
    await first.goto(project.url);
    const pulsing = await first.waitForFunction(() => {
      const nodes = [...document.querySelectorAll('.react-flow__node')];
      const loading = nodes.filter((node) => node.getAnimations({ subtree: true }).some((one) => one.animationName === 'loading-pulse'));
      return loading.length >= 3 ? loading.length : false;
    }, null, { timeout: 15_000 }).then((handle) => handle.jsonValue(), () => 0);
    seen[`pulse_${theme}`] = { pulsing };
    check(`${theme}_boxes_pulse_while_their_previews_are_made`, pulsing >= 3, pulsing);
    if (process.env.SHOT && pulsing) {
      // At the pulse's brightest, then at rest: what a person sees between.
      for (const [phase, at] of [['peak', 0.999], ['rest', 0]]) {
        await first.evaluate((at) => {
          for (const one of document.getAnimations()) {
            if (one.animationName !== 'loading-pulse') continue;
            const length = one.effect.getComputedTiming().duration;
            one.pause();
            one.currentTime = at * length;
          }
        }, at);
        await first.screenshot({ path: `${process.env.SHOT}-pulse-${theme}-${phase}.png` });
      }
    }
    await first.evaluate(() => { for (const one of document.getAnimations()) if (one.animationName === 'loading-pulse') one.play(); });
    await opening(`backfill_${theme}`, first, wire, project, { go: false, bars: false });
    await context.close();
  }

  // Its frames within ~100 ms of its state (Paul's), over every open of it: the middle one, and none past three times that.
  {
    const sorted = [...framed].sort((a, b) => a - b);
    seen.frames = { each_ms: framed, median_ms: sorted[sorted.length >> 1] ?? null };
    check(`frames_within_${BAR.frames_all_ms}ms_of_its_state_the_middle_open`, sorted.length > 0 && sorted[sorted.length >> 1] <= BAR.frames_all_ms, seen.frames);
    check(`frames_within_${3 * BAR.frames_all_ms}ms_of_its_state_every_open`, sorted.length > 0 && sorted.at(-1) <= 3 * BAR.frames_all_ms, seen.frames);
  }

  // Three tabs opening it at once, none of its thumbnails stored (as tabs reopen after Calcifer restarts on a new build): each is made in
  // the background, and nothing a tab asks waits behind them for one of the browser's six connections to Calcifer.
  {
    const project = projects.plain;
    rmSync(join(project.view, '.thumbs'), { recursive: true, force: true });
    rmSync(join(homedir(), '.cozy', 'cache', 'thumbs'), { recursive: true, force: true });
    const { context, wire, tab, first } = await fresh();
    during = 'three_at_once';
    const [second, third] = [await tab('b'), await tab('c')];
    const from = wire.now;
    await Promise.all([first, second, third].map((page) => page.goto(project.url)));
    const states = await Promise.all([first, second, third].map((page) => page.waitForFunction((id) => {
      const read = performance.getEntriesByType('resource').find((entry) => new URL(entry.name).pathname === `/v1/projects/${encodeURIComponent(id)}` && !new URL(entry.name).search && entry.responseEnd > 0);
      return read ? Math.round(read.responseEnd) : false;
    }, project.id, { timeout: WAIT_MS }).then((handle) => handle.jsonValue(), () => null)));
    await first.waitForTimeout(3000);
    const result = { state_ms: states, wire: wire.since(from) };
    seen.three_at_once = result;
    check(`three_at_once_each_state_within_${BAR.previews_warm_ms}ms`, states.every((at) => at !== null && at <= BAR.previews_warm_ms), result);
    check(`three_at_once_no_call_waits_${BAR.frames_first_ms}ms_for_a_connection`, (result.wire.call_wait_ms?.most ?? 0) <= BAR.frames_first_ms, result.wire);
    await context.close();
  }

  // Zoomed out from 100%: each frame of the zoom draws every box in its view.
  {
    const { context, first } = await fresh();
    during = 'zoom out';
    await first.goto(projects.plain.url);
    await first.waitForFunction((n) => Object.values(window.__opens?.nodes ?? {}).filter((row) => row.preview !== null).length >= n, PICTURES, { timeout: WAIT_MS }).catch(() => {});
    await first.keyboard.press('Shift+Digit0');
    await first.waitForTimeout(1200);
    const watching = first.evaluate(FRAMES, 900);
    await first.keyboard.press('f');
    const { pane, frames } = await watching;
    const heights = {};
    for (const frame of frames) {
      const scale = Number(/matrix\(([^,]+)/.exec(frame.transform ?? '')?.[1] ?? 1);
      for (const [name, box] of Object.entries(frame.drawn)) heights[name] = Math.max(heights[name] ?? 0, box[3] / scale);
    }
    const tall = quantile(Object.values(heights).sort((a, b) => a - b), 0.5) ?? W;
    const missing = [];
    let zooms = new Set();
    for (const frame of frames) {
      const matrix = /matrix\(([^)]+)\)/.exec(frame.transform ?? '')?.[1]?.split(',').map(Number);
      if (!matrix) continue;
      const [scale, , , , tx, ty] = matrix;
      zooms.add(scale.toFixed(3));
      const origin = [pane[0] + tx, pane[1] + ty];
      for (let i = 0; i < PICTURES; i++) {
        const { x, y } = placeOf(i);
        const box = [origin[0] + x * scale, origin[1] + y * scale, W * scale, tall * scale];
        const inside = box[0] >= pane[0] && box[1] >= pane[1] && box[0] + box[2] <= pane[0] + pane[2] && box[1] + box[3] <= pane[1] + pane[3];
        if (inside && !frame.drawn[named(i)]) missing.push({ at: Math.round(frame.at), zoom: +scale.toFixed(3), name: named(i) });
      }
    }
    seen.zoom_out = { frames: frames.length, zooms: zooms.size, missing: missing.length, first_missing: missing.slice(0, 8) };
    check('zooming_out_moved_the_view', zooms.size >= 2, seen.zoom_out);
    check('zooming_out_draws_every_box_in_view_in_every_frame', zooms.size >= 2 && missing.length === 0, seen.zoom_out);

    // A file landing while it's open: its thumbnail made as it lands, and drawn soon after.
    during = 'a picture landing';
    const before = thumbnails(stores(projects.plain));
    const landing = Date.now();
    const value = await importInto(projects.plain, paths[PICTURES], named(PICTURES));
    const landed = Date.now();
    projectJson(projects.plain.folder, 'PATCH', '/canvas', { cards: { [value.name]: placeOf(PICTURES) } });
    const shownAt = await first.waitForFunction((name) => {
      const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector(`.card[data-asset="${name}"]`));
      const host = node?.querySelector('.buffered-image');
      const img = host?.querySelector('img');
      return host?.dataset.displayedSha && img?.complete && img.naturalWidth > 0 ? Date.now() : false;
    }, value.name, { timeout: 30_000, polling: 'raf' }).then((handle) => handle.jsonValue(), () => null);
    const madeNow = [...thumbnails(stores(projects.plain))].filter((at) => !before.has(at));
    seen.landed = { import_ms: landed - landing, shown_ms: shownAt === null ? null : shownAt - landed, stored: madeNow.map((at) => at.replace(scratch, '')) };
    check(`a_landed_picture_is_drawn_within_${BAR.landed_ms}ms`, shownAt !== null && shownAt - landed <= BAR.landed_ms, seen.landed);
    check('a_landed_picture_has_its_thumbnail_stored_in_its_project', madeNow.some((at) => at.startsWith(join(projects.plain.view, '.thumbs'))), seen.landed);
    await context.close();
  }

  await browser.close();
  try { cozy('--json', 'vault', '--directory', sealed, 'lock'); } catch {}
  rmSync(scratch, { recursive: true, force: true });
  seen.project = {};
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during, error: String(error?.stack ?? error) }, null, 2));
  process.exit(2);
}
