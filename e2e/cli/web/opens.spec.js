// opens (cl-982; Paul, 11-13: "all of the nodes should appear immediately ... the previews should all appear very quickly; after all, all
// files are already on disc"). Two copies of one project of 50 real pictures of a few MB each, one plaintext and one encrypted: when each
// box's frame is drawn and when each preview is, from the project's first state; how long a request waited for a connection, and the most
// connections to Calcifer at once; with one tab and with two more of the same Calcifer open. A box whose preview is on its way pulses,
// faintly, paper and dark. Its thumbnails are stored in the project as its files land: an open makes none, and one for a file added while
// it's open is drawn soon after it lands. Zoomed out from 100%, every box in the new view is drawn in each frame of the zoom.
// `@solo`: its times are held to bars and it clears the thumbnail cache, so it has the job to itself; that job has the filesystem driver
// its encrypted copy needs.
const { createHash, randomBytes } = require('crypto');
const { readdirSync, rmSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const zlib = require('zlib');
const { test: base, expect, canvasIdle } = require('./fixtures');
const { runnerSpeed } = require('./speed');

const PICTURES = 50;
const SIDE = 1200;
const COLUMNS = 10;
const [W, DX, DY] = [240, 280, 320];
/** Long enough for the old build's stall to show whole, short enough that the cell ends. */
const WAIT_MS = 75_000;
const VIEWPORT = { width: 1600, height: 1000 };
// What Paul asked (11-13), on the fast hosted runners; each is held times this runner's speed against those (speed.js).
const BARS = { frames_first_ms: 100, frames_all_ms: 100, previews_cold_ms: 2000, previews_warm_ms: 1000, landed_ms: 2000 };
const soft = expect.configure({ soft: true });

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

/** Previews drawn in a page, of the 50. */
const previewsDrawn = (page) => page.evaluate((n) => Object.entries(window.__opens?.nodes ?? {}).filter(([name, row]) => /^pic_\d+$/.test(name) && Number(name.slice(4)) < n && row.preview !== null).length, PICTURES);

/** Each file a thumbnail is kept as under `dirs`: what's made while a project opens. */
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

const ms = (value) => (value === null || value === undefined ? null : Math.round(value));
const quantile = (values, q) => (values.length ? values[Math.min(values.length - 1, Math.floor(q * values.length))] : null);

const test = base.extend({
  /** The encrypted copy, made in a folder of the suite's ({id, code, path, view}): locked again before its folder goes. */
  sealed: async ({ api, cli, removeAfter }, use) => {
    const opened = await api.post('/v1/projects', { path: join(process.env.OUT, 'web', 'projects', `web-opens-sealed-${randomBytes(4).toString('hex')}`), passphrase: randomBytes(18).toString('base64url') });
    removeAfter(opened.path);
    const vault = await cli.json('vault', '--directory', opened.path, 'status');
    await use({ id: opened.id, code: opened.code, path: opened.path, vault, view: vault.view ?? vault.vault?.view });
    const at = `/v1/projects/${encodeURIComponent(opened.id)}/vault`;
    if ((await api.get(at)).state === 'locked') return;
    await cli('--json', 'vault', '--directory', opened.path, 'lock');
    await expect.poll(async () => (await api.get(at)).state, { message: `${opened.path} locked before it's removed`, timeout: 60_000 }).toBe('locked');
  },
});

test('opens', { tag: '@solo' }, async ({ project, sealed, api, newContext, signIn }, testInfo) => {
  const speed = runnerSpeed();
  const BAR = Object.fromEntries(Object.entries(BARS).map(([name, ms]) => [name, Math.round(ms * speed.factor)]));
  testInfo.annotations.push({ type: 'runner', description: JSON.stringify({ ...speed, bars: BAR }) });
  /** What each part saw, attached to the report. */
  const seen = {};
  const projects = {
    plain: { id: project.id, code: project.code, folder: project.path, view: project.path },
    sealed: { id: sealed.id, code: sealed.code, folder: sealed.path, view: sealed.view },
  };
  for (const one of Object.values(projects)) one.url = `${api.origin}/p/${one.code}`;
  const at = (one, rest = '') => `/v1/projects/${encodeURIComponent(one.id)}${rest}`;
  /** A picture sent as the page sends one (`PUT /v1/project-files`), then imported as `name`. */
  const importInto = async (one, bytes, name) => {
    const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
    await api.call('PUT', `/v1/project-files/${digest}?project=${encodeURIComponent(one.id)}`, bytes, { type: 'application/octet-stream' });
    return api.post(at(one, '/imports'), { source: digest, name, exact: true });
  };
  // Where a thumbnail can be kept: the project's own store, and the caches before it.
  const stores = (one) => [join(one.view, '.thumbs'), join(homedir(), '.cozy', 'cache', 'thumbs'), join(one.view, '.vault-calcifer', 'thumbs')];
  const pictures = [];

  await test.step('two copies of 50 pictures, plaintext and encrypted, each thumbnail stored as its file lands', async () => {
    expect.soft(sealed.vault, 'the sealed copy is encrypted and unlocked').toMatchObject({ encrypted: true, state: 'unlocked' });
    expect(sealed.view, "the sealed copy's readable view").toBeTruthy();
    const made = Date.now();
    for (let i = 0; i <= PICTURES; i++) pictures.push(picture(i));
    seen.fixture = { pictures: PICTURES, mb: Math.round(pictures[0].length / 1e5) / 10, made_ms: Date.now() - made };
    const importing = Date.now();
    for (const [kind, one] of Object.entries(projects)) {
      const cards = {};
      for (let i = 0; i < PICTURES; i++) cards[(await importInto(one, pictures[i], named(i))).name] = placeOf(i);
      await api.patch(at(one, '/canvas'), { cards });
      // Its view kept in it, as the page keeps one: the whole grid.
      await api.patch(at(one, '/view'), { viewport: FIT });
      seen.fixture[`${kind}_imported_ms`] = Date.now() - importing;
    }
    const storing = Date.now();
    let stored;
    await soft.poll(() => {
      stored = Object.fromEntries(Object.entries(projects).map(([kind, one]) => [kind, thumbnails([join(one.view, '.thumbs')]).size]));
      return Object.values(stored).every((count) => count >= PICTURES);
    }, { message: 'every landed picture has its thumbnail stored in its project', timeout: 60_000, intervals: [250] }).toBe(true);
    seen.stored = { ...stored, ms: Date.now() - storing };
  });

  /** A browser of its own, its cache cold, and its first tab signed in. */
  const fresh = async (theme = 'paper') => {
    const context = await newContext({ viewport: VIEWPORT });
    await context.addInitScript(SAMPLER);
    await context.addInitScript((theme) => {
      try { localStorage.setItem('cozy.theme', theme); } catch {}
    }, theme);
    const wire = new Wire(api.origin);
    // Each tab signs in: a session is the tab's.
    const tab = async (label) => {
      const page = await context.newPage();
      await wire.watch(page, label);
      await signIn(page, `${api.origin}/`);
      await expect(page.locator('.top')).toBeVisible();
      return page;
    };
    return { context, wire, tab, first: await tab('a') };
  };

  // When each open drew all of its frames on screen, after its state.
  const framed = [];
  /** `page` opened on `one` (or reloaded): when its boxes and previews were drawn, against the bars of a `cold` or warm open. */
  const opening = async (label, page, wire, one, { cold, reload = false, others = [], go = true, bars = true } = {}) => {
    const before = go ? thumbnails(stores(one)) : new Set();
    const from = wire.now;
    // Probe (cl-973 opens stutter): a CPU profile of each open, sampled every 100 µs.
    const profiler = await page.context().newCDPSession(page);
    await profiler.send('Profiler.enable');
    await profiler.send('Profiler.setSamplingInterval', { interval: 100 });
    await profiler.send('Profiler.start');
    if (reload) await page.reload();
    else if (go) await page.goto(one.url);
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
          reads: api.map((entry) => [`${path(entry).replace(/project-[0-9a-f]+/, 'P')}${new URL(entry.name).search.replace(/media=[^&]+&?/, '')}`, Math.round(entry.startTime), Math.round(entry.responseEnd)]).sort((a, b) => a[2] - b[2]).slice(0, 24),
          long: (opens?.long ?? []).filter(({ at }) => at < 3000),
        },
        waiting: rows.filter(([, row]) => row.waiting > 0).length,
        pulsing: rows.filter(([, row]) => row.waiting > 0 && row.pulsing === row.waiting).length,
        misses: opens?.misses ?? [],
      };
    }, { id: one.id, pictures: PICTURES });
    let now;
    await soft.poll(async () => {
      now = await read();
      return now.previews.length >= PICTURES || (now.on_screen > 0 && now.on_screen_previews.length >= now.on_screen);
    }, { message: `${label}: every preview on screen drawn`, timeout: WAIT_MS, intervals: [100] }).toBe(true);
    const { profile } = await profiler.send('Profiler.stop');
    const origin = await page.evaluate(() => performance.timeOrigin);
    await testInfo.attach(`profile-${label}`, { body: JSON.stringify({ origin, read: now, profile }), contentType: 'application/json' });
    await profiler.detach();
    const made = [...thumbnails(stores(one))].filter((file) => !before.has(file)).length;
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
    const said = (what) => `${label}: ${what}: ${JSON.stringify({ ...result, trace: undefined, preview_times_ms: undefined })}`;
    expect.soft(result.pulsed_while_waiting, said('every box waiting for its preview pulses')).toBe(result.waited_for_preview);
    if (!bars) {
      expect.soft(result.on_screen > 0 && result.on_screen_previews_ms !== null, said('every preview on screen drawn')).toBe(true);
      return result;
    }
    const bar = cold ? BAR.previews_cold_ms : BAR.previews_warm_ms;
    // Every frame on screen is drawn in one go: its first and its last. How soon is held over all the opens (below), as one open's frames
    // can wait out a frame of the page's other work on a runner that rasters in software.
    expect.soft(result.all_frames_ms, said('every frame on screen drawn')).not.toBeNull();
    framed.push(result.all_frames_ms ?? Infinity);
    expect.soft(result.on_screen > 0 && result.on_screen_previews_ms !== null && result.on_screen_previews_ms <= bar, said(`every preview on screen within ${bar} ms`)).toBe(true);
    if (cold) expect.soft(made, said('a cold open makes no thumbnail')).toBe(0);
    return result;
  };

  for (const kind of ['plain', 'sealed']) {
    await test.step(`${kind}: opened cold, then warm, then fitted`, async () => {
      const one = projects[kind];
      const { context, wire, first } = await fresh();
      await opening(`${kind}_cold`, first, wire, one, { cold: true });
      await opening(`${kind}_warm`, first, wire, one, { cold: false, reload: true });
      // All of it in view: each of the 50 read from its store (an encrypted one's from its encrypted store, never the browser's cache).
      const fitting = await first.evaluate(() => performance.now());
      await first.keyboard.press('f');
      let fitted = null;
      await soft.poll(async () => (fitted = await first.evaluate((n) => {
        const times = Object.entries(window.__opens?.nodes ?? {}).filter(([name]) => /^pic_\d+$/.test(name) && Number(name.slice(4)) < n).map(([, row]) => row.preview);
        return times.length >= n && times.every((at) => at !== null) ? Math.max(...times) : null;
      }, PICTURES)) !== null, { message: `${kind}: fitted, all ${PICTURES} previews drawn`, timeout: WAIT_MS, intervals: [100] }).toBe(true);
      seen[`${kind}_fitted`] = { all_previews_ms: fitted === null ? null : Math.max(0, Math.round(fitted - fitting)) };
      expect.soft(fitted !== null && fitted - fitting <= BAR.previews_warm_ms, `${kind}: fitted, all ${PICTURES} previews within ${BAR.previews_warm_ms} ms: ${JSON.stringify(seen[`${kind}_fitted`])}`).toBe(true);
      await context.close();
    });
  }

  for (const [kind, other] of [['plain', 'sealed'], ['sealed', 'plain']]) {
    await test.step(`${kind}: opened cold, then warm, with two other tabs of this Calcifer open on the other copy`, async () => {
      const { context, wire, tab, first } = await fresh();
      await first.goto(projects[other].url);
      const second = await tab('b');
      await second.goto(projects[other].url);
      for (const page of [first, second]) await soft.poll(() => previewsDrawn(page), { message: `the other tabs' ${PICTURES} previews drawn`, timeout: WAIT_MS }).toBeGreaterThanOrEqual(PICTURES);
      const third = await tab('c');
      await opening(`${kind}_three_tabs_cold`, third, wire, projects[kind], { cold: true, others: [first, second] });
      await opening(`${kind}_three_tabs_warm`, third, wire, projects[kind], { cold: false, reload: true, others: [first, second] });
      await context.close();
    });
  }

  // An old project's thumbnails, none stored: each made in the background as it's asked for, its box pulsing until it's drawn.
  for (const theme of ['paper', 'dark']) {
    await test.step(`${theme}: none stored, each box pulses while its preview is made`, async () => {
      const one = projects.plain;
      rmSync(join(one.view, '.thumbs'), { recursive: true, force: true });
      rmSync(join(homedir(), '.cozy', 'cache', 'thumbs'), { recursive: true, force: true });
      const { context, wire, first } = await fresh(theme);
      await first.goto(one.url);
      let pulsing = 0;
      await soft.poll(async () => (pulsing = await first.evaluate(() => [...document.querySelectorAll('.react-flow__node')].filter((node) => node.getAnimations({ subtree: true }).some((held) => held.animationName === 'loading-pulse')).length)), { message: `${theme}: boxes pulse while their previews are made`, timeout: 15_000, intervals: [50] }).toBeGreaterThanOrEqual(3);
      seen[`pulse_${theme}`] = { pulsing };
      await opening(`backfill_${theme}`, first, wire, one, { go: false, bars: false });
      await context.close();
    });
  }

  await test.step(`frames within ${BAR.frames_all_ms} ms of its state, the middle open, and ${3 * BAR.frames_all_ms} ms every open`, async () => {
    const sorted = [...framed].sort((a, b) => a - b);
    seen.frames = { each_ms: framed, median_ms: sorted[sorted.length >> 1] ?? null };
    expect.soft(sorted.length > 0 && sorted[sorted.length >> 1] <= BAR.frames_all_ms, `the middle open: ${JSON.stringify(seen.frames)}`).toBe(true);
    expect.soft(sorted.length > 0 && sorted.at(-1) <= 3 * BAR.frames_all_ms, `every open: ${JSON.stringify(seen.frames)}`).toBe(true);
  });

  // Three tabs opening it at once, none of its thumbnails stored (as tabs reopen after Calcifer restarts on a new build): each is made in
  // the background, and nothing a tab asks waits behind them for one of the browser's six connections to Calcifer.
  await test.step('three tabs opening it at once, none stored: no call waits for a connection', async () => {
    const one = projects.plain;
    rmSync(join(one.view, '.thumbs'), { recursive: true, force: true });
    rmSync(join(homedir(), '.cozy', 'cache', 'thumbs'), { recursive: true, force: true });
    const { context, wire, tab, first } = await fresh();
    const tabs = [first, await tab('b'), await tab('c')];
    const from = wire.now;
    await Promise.all(tabs.map((page) => page.goto(one.url)));
    const states = [];
    for (const page of tabs) {
      let state = null;
      await soft.poll(async () => (state = await page.evaluate((id) => {
        const read = performance.getEntriesByType('resource').find((entry) => new URL(entry.name).pathname === `/v1/projects/${encodeURIComponent(id)}` && !new URL(entry.name).search && entry.responseEnd > 0);
        return read ? Math.round(read.responseEnd) : null;
      }, one.id)) !== null, { message: 'its state read', timeout: WAIT_MS }).toBe(true);
      states.push(state);
    }
    // Each tab's thumbnails made and drawn: every request of theirs is in the log.
    for (const page of tabs) await soft.poll(() => previewsDrawn(page), { message: `each tab's ${PICTURES} previews drawn`, timeout: WAIT_MS }).toBeGreaterThanOrEqual(PICTURES);
    const result = { state_ms: states, wire: wire.since(from) };
    seen.three_at_once = result;
    expect.soft(states.every((took) => took !== null && took <= BAR.previews_warm_ms), `each state within ${BAR.previews_warm_ms} ms: ${JSON.stringify(result)}`).toBe(true);
    expect.soft(result.wire.call_wait_ms?.most ?? 0, `no call waits ${BAR.frames_first_ms} ms for a connection: ${JSON.stringify(result.wire)}`).toBeLessThanOrEqual(BAR.frames_first_ms);
    await context.close();
  });

  await test.step('zoomed out from 100%, every box in view is drawn in every frame; a picture landing while it is open is drawn soon after', async () => {
    const { context, first } = await fresh();
    await first.goto(projects.plain.url);
    await soft.poll(() => previewsDrawn(first), { message: `its ${PICTURES} previews drawn`, timeout: WAIT_MS }).toBeGreaterThanOrEqual(PICTURES);
    await first.keyboard.press('Shift+Digit0');
    await canvasIdle(first);
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
    const zooms = new Set();
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
    expect.soft(zooms.size, `zooming out moved the view: ${JSON.stringify(seen.zoom_out)}`).toBeGreaterThanOrEqual(2);
    expect.soft(missing, `zooming out draws every box in view in every frame: ${JSON.stringify(seen.zoom_out)}`).toEqual([]);

    // A file landing while it's open: its thumbnail made as it lands, and drawn soon after.
    const name = named(PICTURES);
    await first.evaluate((name) => {
      const look = () => {
        const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector(`.card[data-asset="${name}"]`));
        const host = node?.querySelector('.buffered-image');
        const img = host?.querySelector('img');
        if (host?.dataset.displayedSha && img?.complete && img.naturalWidth > 0) window.__landed = Date.now();
        else requestAnimationFrame(look);
      };
      requestAnimationFrame(look);
    }, name);
    const before = thumbnails(stores(projects.plain));
    const landing = Date.now();
    const value = await importInto(projects.plain, pictures[PICTURES], name);
    const landed = Date.now();
    await api.patch(at(projects.plain, '/canvas'), { cards: { [value.name]: placeOf(PICTURES) } });
    let shownAt = null;
    await soft.poll(async () => (shownAt = await first.evaluate(() => window.__landed ?? null)) !== null, { message: 'the landed picture drawn', timeout: 30_000 }).toBe(true);
    const madeNow = [...thumbnails(stores(projects.plain))].filter((file) => !before.has(file));
    seen.landed = { import_ms: landed - landing, shown_ms: shownAt === null ? null : shownAt - landed, stored: madeNow.map((file) => file.replace(projects.plain.view, '')) };
    expect.soft(shownAt !== null && shownAt - landed <= BAR.landed_ms, `a landed picture is drawn within ${BAR.landed_ms} ms: ${JSON.stringify(seen.landed)}`).toBe(true);
    expect.soft(madeNow.some((file) => file.startsWith(join(projects.plain.view, '.thumbs'))), `a landed picture has its thumbnail stored in its project: ${JSON.stringify(seen.landed)}`).toBe(true);
    await context.close();
  });

  await testInfo.attach('opens', { body: JSON.stringify(seen, null, 2), contentType: 'application/json' });
});
