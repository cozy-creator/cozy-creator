// What the page costs while nothing happens (xs-189; Paul, 11-12: "nonsensical code that runs in a loop in the background doing
// unnecessary things / polling"): a project open on an idle canvas, for a window in view, one in a tab behind another, and one in view with
// motion kept down (the paper sky's drift stopped). It counts the
// timers, frames and scheduled work that ran (by where each was set), the requests, the page's main-thread time, the browser's processes'
// CPU and Calcifer's. An idle page asks Calcifer nothing, and nothing set in it runs over and over, in view or hidden.
//   node pageidle.js (shard web-pageidle): signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT: playwright-core's directory;
// $CHROME: the browser; $SHOT: where its screenshots go; $IDLE_MS: each window (60000). Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readdirSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { basename, join } = require('path');
const { boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, signIn, STEP_MS, until, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}
const IDLE_MS = Number(process.env.IDLE_MS ?? 60_000);
/** Quiet this long, with no request, before a window starts: what opening the project asked for has been answered. A page that never
 *  goes quiet (one that polls) is measured after `SETTLE_MS`. */
const QUIET_MS = 10_000;
const SETTLE_MS = 45_000;
/** A place that set something running this many times in one window is a loop. */
const LOOP = 3;

/** In the page's own world, before its scripts: every timer, frame and message counted as it runs, under where it was set. */
function counting() {
  const idle = (window.__idle = { on: false, ran: {}, at: {} });
  const where = () => (new Error().stack ?? '').split('\n').slice(3).map((line) => line.trim()).find((line) => line.startsWith('at ')) ?? '?';
  const ran = (kind, at) => {
    if (!idle.on) return;
    idle.ran[kind] = (idle.ran[kind] ?? 0) + 1;
    idle.at[`${kind} ${at}`] = (idle.at[`${kind} ${at}`] ?? 0) + 1;
  };
  for (const [kind, name] of [['timeout', 'setTimeout'], ['interval', 'setInterval']]) {
    const original = window[name];
    window[name] = function (callback, ...rest) {
      if (typeof callback !== 'function') return original.call(window, callback, ...rest);
      const at = where();
      return original.call(window, function (...args) {
        ran(kind, at);
        return callback.apply(this, args);
      }, ...rest);
    };
  }
  const frame = window.requestAnimationFrame;
  window.requestAnimationFrame = function (callback) {
    const at = where();
    return frame.call(window, (time) => {
      ran('frame', at);
      return callback(time);
    });
  };
  // React's scheduler and the tabs' channel: work queued, whatever woke it.
  for (const [kind, Kind] of [['message', MessagePort], ['broadcast', BroadcastChannel]]) {
    const post = Kind.prototype.postMessage;
    Kind.prototype.postMessage = function (...args) {
      ran(kind, where());
      return post.apply(this, args);
    };
  }
}

/** Playwright keeps every tab "visible" (playwright-core 1.55.0's crPage.js): undone on the tab's own driver session, as vault.js does,
 *  so Chrome says a tab behind another is hidden. */
async function realVisibility(tab) {
  const owner = tab._connection?.toImpl?.(tab)?.delegate?._mainFrameSession?._client;
  if (!owner?.send) throw new Error('a hidden tab needs the pinned driver session (playwright-core 1.55.0)');
  await owner.send('Emulation.setFocusEmulationEnabled', { enabled: false });
}

/** Calcifer's CPU (ms) and context switches, from /proc, where there is one. */
function calciferUse() {
  if (process.platform !== 'linux') return null;
  for (const entry of readdirSync('/proc')) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const argv = readFileSync(`/proc/${entry}/cmdline`, 'utf8').split('\0').filter((arg) => arg && arg !== '--json');
      if (!basename(argv[0] ?? '').startsWith('cozy') || argv.slice(1).join(' ') !== 'calcifer') continue;
      const stat = readFileSync(`/proc/${entry}/stat`, 'utf8');
      const rest = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
      const status = readFileSync(`/proc/${entry}/status`, 'utf8');
      const switches = status.split('\n').filter((line) => /ctxt_switches:/.test(line)).reduce((sum, line) => sum + Number(line.split(':')[1]), 0);
      // Ticks are 1/100 s on Linux.
      return { pid: Number(entry), cpu_ms: (Number(rest[11]) + Number(rest[12])) * 10, switches };
    } catch {}
  }
  return null;
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  await context.addInitScript(counting);
  const page = await context.newPage();
  await realVisibility(page);
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  // A look at each window, never its verdict: a tab just back in front can take its time to draw.
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png`, timeout: 15_000 }).catch(() => {}) : Promise.resolve());
  const asked = [];
  let lastAsk = Date.now();
  context.on('request', (request) => {
    lastAsk = Date.now();
    const url = new URL(request.url());
    asked.push({ method: request.method(), path: url.pathname });
  });

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  // An ordinary project, as a person leaves one: made on the page, two pictures on its canvas.
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-idle-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };
  const folder = mkdtempSync(join(tmpdir(), 'web-idle-'));
  for (const [name, shade, x] of [['meadow', 90, 100], ['harbour', 170, 520]]) {
    writeFileSync(join(folder, `${name}.png`), png(shade, 480, 320));
    const imported = await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(folder, `${name}.png`)), name, exact: true, at: { x, y: 120 } }) });
    if (imported.status >= 300) throw new Error(`import ${name}: ${imported.status} ${JSON.stringify(imported.body)}`);
    await boxOf(page, name).waitFor();
  }
  await fitCanvas(page);

  const cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  const browserCdp = await browser.newBrowserCDPSession().catch(() => null);
  const metrics = async () => Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(({ name, value }) => [name, value]));
  const processes = async () => {
    if (!browserCdp) return null;
    try {
      const { processInfo } = await browserCdp.send('SystemInfo.getProcessInfo');
      const by = {};
      for (const one of processInfo) by[one.type] = (by[one.type] ?? 0) + one.cpuTime;
      return by;
    } catch {
      return null;
    }
  };

  /** What `IDLE_MS` with nothing done costs, after the page has been quiet for `QUIET_MS`. */
  const measure = async (name) => {
    const quiet = await until(`${QUIET_MS / 1000} s without a request`, async () => Date.now() - lastAsk, (quiet) => quiet >= QUIET_MS, SETTLE_MS).then(() => true, () => false);
    const from = asked.length;
    const [m0, p0, c0] = [await metrics(), await processes(), calciferUse()];
    await page.evaluate(() => { Object.assign(window.__idle, { on: true, ran: {}, at: {} }); });
    const began = Date.now();
    await new Promise((done) => setTimeout(done, IDLE_MS));
    const counted = await page.evaluate(() => { window.__idle.on = false; return { ran: window.__idle.ran, at: window.__idle.at, visibility: document.visibilityState }; });
    const ms = Date.now() - began;
    const [m1, p1, c1] = [await metrics(), await processes(), calciferUse()];
    const requests = asked.slice(from).map(({ method, path }) => `${method} ${path}`);
    const by = {};
    for (const request of requests) by[request] = (by[request] ?? 0) + 1;
    const delta = (key) => Math.round((m1[key] - m0[key]) * 1000);
    const said = {
      ms,
      quiet_first: quiet,
      visibility: counted.visibility,
      requests: requests.length,
      requests_by: by,
      ran: counted.ran,
      // Busiest first: a site's count and the bundle's text where it was set.
      sites: Object.entries(counted.at).sort((a, b) => b[1] - a[1]).slice(0, 12).map(([at, n]) => ({ at, n })),
      main_thread_ms: { task: delta('TaskDuration'), script: delta('ScriptDuration'), layout: delta('LayoutDuration'), style: delta('RecalcStyleDuration') },
      layouts: m1.LayoutCount - m0.LayoutCount,
      style_recalcs: m1.RecalcStyleCount - m0.RecalcStyleCount,
      processes_cpu_ms: p0 && p1 ? Object.fromEntries(Object.keys(p1).map((type) => [type, Math.round((p1[type] - (p0[type] ?? 0)) * 1000)])) : null,
      calcifer: c0 && c1 && c0.pid === c1.pid ? { cpu_ms: c1.cpu_ms - c0.cpu_ms, switches: c1.switches - c0.switches } : null,
    };
    // Where in the bundle each busy site is: its function's text, as the minified bundle has it.
    for (const site of said.sites) {
      const where = site.at.match(/(https?:\/\/[^\s)]+):(\d+):(\d+)\)?$/);
      if (!where) continue;
      const text = await page.evaluate((url) => fetch(url).then((answer) => answer.text()).catch(() => ''), where[1]);
      const line = text.split('\n')[Number(where[2]) - 1] ?? '';
      site.code = line.slice(Math.max(0, Number(where[3]) - 80), Number(where[3]) + 80);
      site.at = site.at.replace(base, '');
    }
    seen[name] = said;
    await shot(name);
    return said;
  };

  const shown = await measure('visible');
  check('visible_asks_nothing', shown.requests === 0, shown.requests_by);
  check('visible_runs_no_loop', shown.sites.every(({ n }) => n < LOOP), shown.sites.filter(({ n }) => n >= LOOP));

  // Behind another tab, as a person leaves it for a while.
  const front = await context.newPage();
  await realVisibility(front);
  await front.goto('about:blank');
  await front.bringToFront();
  await until('the project tab hidden', () => page.evaluate(() => document.visibilityState), (state) => state === 'hidden', 10_000);
  const hidden = await measure('hidden');
  check('hidden_is_hidden', hidden.visibility === 'hidden', hidden.visibility);
  check('hidden_asks_nothing', hidden.requests === 0, hidden.requests_by);
  check('hidden_runs_no_loop', hidden.sites.every(({ n }) => n < LOOP), hidden.sites.filter(({ n }) => n >= LOOP));
  await page.bringToFront();
  await until('the project tab shown again', () => page.evaluate(() => document.visibilityState), (state) => state === 'visible', 10_000);
  check('canvas_still_live', await boxOf(page, 'harbour').isVisible());

  // In view with motion kept down: the paper sky's clouds stop, so this window less the first is what their drift costs.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const still = await measure('still');
  check('still_asks_nothing', still.requests === 0, still.requests_by);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
