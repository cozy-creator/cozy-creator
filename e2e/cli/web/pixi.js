// One production Pixi smoke cell on this computer's existing Cozy, in its default home. No backend responses are mocked.
// COZY and CHROME name the installed CLI and Chrome; PLAYWRIGHT names playwright-core; PIXI_OUT is an owned receipt parent.
// Optional CANDIDATE_DIST serves ONLY that built UI's files at Calcifer's actual origin, preserving its live document security headers.
// Optional CHROME_ARGS is a JSON array of hardware-specific browser arguments. Software WebGL is a failure, never a fallback.
// The generated project and receipts are retained, including after failure. This cell neither replaces nor stops Calcifer.
const { chromium } = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, writeFileSync } = require('node:fs');
const { homedir } = require('node:os');
const { extname, join, resolve } = require('node:path');
const { inflateSync } = require('node:zlib');
const { png, MP4, openCanvas } = require('./page');

const STEP = 30_000;
const CLI_BUDGET = 120_000; // Fixture setup is separate from interaction timing on the shared machine.
const failures = [], seen = {}, secrets = [];
let browser, page, out, during = 'configuration';
const clean = (value) => secrets.reduce((text, secret) => text.replaceAll(secret, '[redacted]'), String(value));
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
function check(name, ok, detail) {
  during = name;
  seen[name] = ok ? true : detail ?? false;
  if (!ok) failures.push(name);
  process.stderr.write(`[pixi-smoke] ${name}: ${ok ? 'pass' : 'FAIL'}\n`);
}
async function until(name, read, accept, ms = STEP) {
  during = name;
  const end = Date.now() + ms;
  let value;
  do {
    value = await read();
    if (accept(value)) return value;
    await new Promise(done => setTimeout(done, 100));
  } while (Date.now() < end);
  throw new Error(`${name}: ${JSON.stringify(value)}`);
}

/** Decode Chrome's 8-bit RGB/RGBA screenshot in Node, independently of browser Canvas2D/GPU readback. */
function pixels(bytes) {
  if (!bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) throw new Error('screenshot is not PNG');
  const chunks = [];
  let width, height, channels;
  for (let at = 8; at < bytes.length;) {
    const length = bytes.readUInt32BE(at), type = bytes.toString('ascii', at + 4, at + 8), data = bytes.subarray(at + 8, at + 8 + length);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0); height = data.readUInt32BE(4);
      channels = data[9] === 2 ? 3 : data[9] === 6 ? 4 : 0;
      if (data[8] !== 8 || !channels || data[12] !== 0) throw new Error('unsupported screenshot PNG format');
    }
    if (type === 'IDAT') chunks.push(data);
    at += length + 12;
  }
  const raw = inflateSync(Buffer.concat(chunks)), stride = width * channels, decoded = Buffer.alloc(stride * height);
  if (raw.length !== (stride + 1) * height) throw new Error('screenshot PNG length differs');
  const paeth = (a, b, c) => { const p = a + b - c, x = Math.abs(p - a), y = Math.abs(p - b), z = Math.abs(p - c); return x <= y && x <= z ? a : y <= z ? b : c; };
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    if (filter > 4) throw new Error('unknown screenshot PNG filter');
    for (let x = 0; x < stride; x++) {
      const at = y * stride + x, a = x >= channels ? decoded[at - channels] : 0, b = y ? decoded[at - stride] : 0, c = y && x >= channels ? decoded[at - stride - channels] : 0;
      decoded[at] = raw[y * (stride + 1) + 1 + x] + [0, a, b, Math.floor((a + b) / 2), paeth(a, b, c)][filter];
    }
  }
  return (x, y) => {
    x = Math.round(x); y = Math.round(y);
    if (x < 0 || x >= width || y < 0 || y >= height) throw new Error('pixel probe is offscreen');
    return [...decoded.subarray((y * width + x) * channels, (y * width + x) * channels + 3)];
  };
}

/** Static delivery only: API, session, project-file and media requests always reach the real origin unchanged. */
async function candidate(context, origin, directory) {
  if (!directory) return;
  const dist = realpathSync(directory), files = new Map();
  const walk = (path, prefix = '') => {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const name = `${prefix}/${entry.name}`, file = join(path, entry.name);
      if (entry.isDirectory()) walk(file, name);
      else if (entry.isFile()) files.set(name, readFileSync(file));
      else throw new Error(`candidate contains a non-regular file: ${name}`);
    }
  };
  walk(dist);
  const index = files.get('/index.html');
  if (!index || /@vite\/client|__COZY_DEVELOPMENT_NONCE__/.test(index.toString())) throw new Error('CANDIDATE_DIST must contain the built UI');
  const mime = { '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.webp': 'image/webp', '.woff': 'font/woff', '.woff2': 'font/woff2', '.wasm': 'application/wasm', '.json': 'application/json', '.txt': 'text/plain', '.ico': 'image/x-icon' };
  let security = null;
  seen.candidate = { dist, files: [...files].map(([path, body]) => ({ path, sha256: digest(body), bytes: body.length })), static_requests: [], documents: [] };
  await context.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.origin !== origin || url.pathname.startsWith('/v1') || request.method() !== 'GET') return route.continue();
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) {
      const response = await route.fetch();
      const headers = response.headers();
      if (!response.ok() || !headers['content-type']?.includes('text/html') || !headers['content-security-policy']) {
        await route.fulfill({ response });
        return;
      }
      security = Object.fromEntries(Object.entries(headers).filter(([name]) => ['content-security-policy', 'referrer-policy', 'x-content-type-options', 'cross-origin-opener-policy', 'cross-origin-embedder-policy'].includes(name)));
      seen.candidate.documents.push({ path: url.pathname, status: response.status(), csp_sha256: digest(headers['content-security-policy']) });
      for (const name of ['content-length', 'content-encoding', 'transfer-encoding', 'etag', 'last-modified']) delete headers[name];
      await route.fulfill({ response, headers: { ...headers, 'cache-control': 'no-store' }, body: index });
      return;
    }
    const body = files.get(decodeURIComponent(url.pathname));
    if (!body || !security) return route.continue();
    seen.candidate.static_requests.push(url.pathname);
    await route.fulfill({ status: 200, headers: { ...security, 'content-type': mime[extname(url.pathname)] ?? 'application/octet-stream', 'cache-control': 'no-store' }, body });
  });
}

(async () => {
  if (!process.env.COZY || !process.env.CHROME || !process.env.PIXI_OUT) throw new Error('COZY, CHROME, PLAYWRIGHT and PIXI_OUT are required');
  if (process.env.COZY_HOME || process.env.PAGE_ORIGIN) throw new Error('this cell uses the existing default-home Calcifer origin');
  const record = join(homedir(), '.cozy/calcifer/calcifer.lock.holder');
  const holder = JSON.parse(readFileSync(record, 'utf8'));
  if (!holder.token || !holder.address || !holder.pid) throw new Error('the default Calcifer holder is incomplete');
  secrets.push(holder.token);
  process.kill(holder.pid, 0);
  const origin = `http://${holder.address}`;
  const sameService = () => {
    const now = JSON.parse(readFileSync(record, 'utf8'));
    if (now.pid !== holder.pid || now.address !== holder.address) throw new Error('Calcifer changed during the cell');
  };
  const api = async path => {
    const response = await fetch(origin + path, { headers: { Authorization: `Bearer ${holder.token}`, 'cozy-project-storage': '3' }, signal: AbortSignal.timeout(STEP) });
    if (!response.ok) throw new Error(`GET ${path}: ${response.status}`);
    return response.json();
  };
  const status = await api('/v1/status');
  seen.backend = { origin, pid: holder.pid, version: status.version };
  mkdirSync(resolve(process.env.PIXI_OUT), { recursive: true });
  out = mkdtempSync(join(resolve(process.env.PIXI_OUT), 'pixi-'));
  const folder = join(out, 'project');
  const cli = (args, cwd = out) => {
    sameService();
    let text;
    try { text = execFileSync(process.env.COZY, args, { cwd, encoding: 'utf8', timeout: CLI_BUDGET, stdio: ['ignore', 'pipe', 'pipe'] }); }
    catch (error) { throw new Error(`cozy ${args[0]} failed (${error.code ?? error.status ?? error.signal ?? 'unknown'}): ${clean(error.stderr || error.message)}`); }
    sameService();
    return JSON.parse(text);
  };
  during = 'create owned project';
  const made = cli(['new', '--unencrypted', '--json', folder]);
  if (!made.created || !made.id) throw new Error('cozy new did not create the owned project');
  seen.project = { id: made.id, code: made.code, folder: made.path };
  const at = `/v1/projects/${encodeURIComponent(made.id)}`;
  const asset = async name => (await api(at)).assets.find(row => row.name === name);
  const placed = async name => (await api(`${at}/canvas`)).cards[name];
  const opened = cli(['open', '--json'], folder);
  const link = new URL(opened.url);
  const code = new URLSearchParams(link.hash.slice(1)).get('code');
  if (!code) throw new Error('cozy open did not return a one-use sign-in code');
  secrets.push(code);
  if (link.origin !== origin) throw new Error('cozy open returned a different origin');
  const extra = JSON.parse(process.env.CHROME_ARGS ?? '[]');
  if (!Array.isArray(extra) || extra.some(arg => typeof arg !== 'string' || /swiftshader|llvmpipe|lavapipe|disable-gpu(?:$|=)/i.test(arg))) throw new Error('CHROME_ARGS must select a hardware renderer');
  seen.browser_args = ['--enable-gpu', '--disable-software-rasterizer', ...extra];
  browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true, args: seen.browser_args, ignoreDefaultArgs: ['--hide-scrollbars'], timeout: STEP });
  seen.browser = browser.version();
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1, serviceWorkers: 'block' });
  page = await context.newPage();
  page.setDefaultTimeout(STEP);
  page.setDefaultNavigationTimeout(STEP);
  const errors = [], violations = [], apiOrigins = new Set();
  seen.pageErrors = errors; seen.cspViolations = violations;
  page.on('pageerror', error => errors.push(clean(error.message)));
  page.on('console', message => { if (message.type() === 'error') errors.push(clean(message.text())); });
  page.on('request', request => { const url = new URL(request.url()); if (url.pathname.startsWith('/v1')) apiOrigins.add(url.origin); });
  await page.exposeFunction('recordCspViolation', value => violations.push(value));
  await page.addInitScript(() => { document.addEventListener('securitypolicyviolation', event => window.recordCspViolation({ directive: event.violatedDirective, disposition: event.disposition })); });
  await candidate(context, origin, process.env.CANDIDATE_DIST);
  const session = page.waitForResponse(response => new URL(response.url()).pathname === '/v1/session' && response.request().method() === 'POST');
  const document = await page.goto(link.href);
  check('real_session_redeemed', (await session).ok());
  check('document_keeps_csp', Boolean(document.headers()['content-security-policy']) && document.headers()['referrer-policy'] === 'no-referrer');
  await page.getByRole('region', { name: 'Project canvas', exact: true }).waitFor();
  const canvas = page.getByRole('region', { name: 'Project canvas', exact: true });
  await until('canvas inspection available', () => canvas.evaluate(el => Boolean(el.canvasInspection)), Boolean);
  const inspect = (method, arg) => canvas.evaluate((el, { method, arg }) => el.canvasInspection?.[method]?.(arg) ?? null, { method, arg });
  const camera = () => inspect('camera');
  const settled = async () => {
    let prior, still = 0;
    await until('camera settled', camera, view => { if (!view) return false; const next = JSON.stringify(view); still = next === prior ? still + 1 : 0; prior = next; return still >= 3; });
  };
  const gpu = await page.locator('canvas.canvas-pixels').evaluate(element => {
    const gl = element.getContext('webgl2') || element.getContext('webgl');
    const info = gl?.getExtension('WEBGL_debug_renderer_info');
    return gl && info ? { vendor: gl.getParameter(info.UNMASKED_VENDOR_WEBGL), renderer: gl.getParameter(info.UNMASKED_RENDERER_WEBGL), version: gl.getParameter(gl.VERSION), dpr: devicePixelRatio } : null;
  });
  seen.gpu = gpu;
  if (!gpu || /swiftshader|llvmpipe|softpipe|lavapipe|software|swrast/i.test(gpu.renderer)) throw new Error('actual canvas hardware WebGL could not be established');
  const cdp = await browser.newBrowserCDPSession();
  const system = await cdp.send('SystemInfo.getInfo');
  seen.gpu_process = { devices: system.gpu.devices, featureStatus: system.gpu.featureStatus };
  await cdp.detach();

  during = 'UI file import';
  const colors = [[228, 46, 56], [42, 93, 230], [30, 180, 70], [245, 191, 39]];
  const image = png(0, 512, 320, false, (x, y) => colors[(y >= 160 ? 2 : 0) + (x >= 256 ? 1 : 0)]);
  writeFileSync(join(out, 'source.png'), image);
  seen.fixture = { sha256: digest(image), width: 512, height: 320 };
  await page.getByLabel('Add a file', { exact: true }).setInputFiles({ name: 'pixi_source.png', mimeType: 'image/png', buffer: image });
  const name = page.getByRole('textbox', { name: 'Name', exact: true });
  await name.waitFor(); await name.fill('pixi_source'); await name.press('Enter');
  await until('import persisted', () => asset('pixi_source'), value => value?.sha256 === `sha256:${digest(image)}`);
  check('upload_stays_in_wander', await page.locator('[data-focused-asset]').count() === 0);
  const fit = async () => {
    await page.getByRole('button', { name: /, more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
    await settled();
  };
  await fit();
  const passive = async () => {
    await page.mouse.move(20, 20);
    await until('native card overlay released', () => page.locator('.canvas-node').count(), count => count === 0);
  };
  const point = async (name, part = 'body') => {
    const box = (await until(`${name} geometry`, () => inspect('card', name), Boolean))[part];
    return { x: box.x + box.width / 2, y: box.y + (part === 'body' ? box.width * 320 / 512 / 2 : box.height / 2) };
  };
  const picture = async (name, label) => {
    await passive();
    const sample = await until(`${label} displayed pixels`, async () => {
      const box = (await inspect('card', name)).body, bytes = await page.screenshot(), read = pixels(bytes);
      const actual = [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]].map(([x, y]) => read(box.x + box.width * x, box.y + box.width * 320 / 512 * y));
      writeFileSync(join(out, `${label}.png`), bytes);
      return { actual, matches: actual.every((color, i) => color.every((channel, c) => Math.abs(channel - colors[i][c]) <= 18)) };
    }, result => result.matches);
    (seen.pixels ??= {})[label] = sample.actual;
    check(label, sample.matches, sample);
  };
  await picture('pixi_source', 'passive_real_pixels');
  let p = await point('pixi_source');
  await page.mouse.click(p.x, p.y);
  await until('single click selected', () => inspect('selectedNames'), names => names.join() === 'pixi_source');
  check('single_click_does_not_focus', await page.locator('[data-focused-asset]').count() === 0);
  await page.keyboard.press('Enter');
  await page.locator('[data-focused-asset="pixi_source"]:not([data-entering])').waitFor();
  check('mouse_selection_then_enter_opens_focus', true);
  await page.keyboard.press('Escape'); await page.locator('[data-focused-asset]').waitFor({ state: 'detached' }); await settled();
  await canvas.focus(); await page.keyboard.press('Escape'); await page.keyboard.press('Enter'); await settled();
  check('cleared_selection_enter_does_not_reopen_old_card', await page.locator('[data-focused-asset]').count() === 0);
  p = await point('pixi_source', 'title');
  await page.mouse.click(p.x, p.y);
  await name.waitFor(); await name.fill('pixi_renamed'); await name.press('Enter');
  await until('rename persisted', () => asset('pixi_renamed'), Boolean);
  check('rename_keeps_identity', !(await asset('pixi_source')) && (await asset('pixi_renamed')).sha256 === `sha256:${digest(image)}`);
  const before = await placed('pixi_renamed'), zoom = (await camera()).zoom;
  p = await point('pixi_renamed');
  await page.mouse.move(p.x, p.y); await page.mouse.down(); await page.mouse.move(p.x + 80, p.y + 55, { steps: 12 }); await page.mouse.up();
  const moved = await until('drag persisted', () => placed('pixi_renamed'), row => Math.abs(row.x - before.x - 80 / zoom) < 3 && Math.abs(row.y - before.y - 55 / zoom) < 3);
  check('drag_saved_world_coordinates', true, moved);
  await passive(); await canvas.focus();
  await page.keyboard.press('Delete');
  await until('delete persisted', () => asset('pixi_renamed'), row => !row);
  await page.keyboard.press('ControlOrMeta+z');
  await until('undo restored asset', () => asset('pixi_renamed'), Boolean);
  const undone = await placed('pixi_renamed');
  check('undo_restores_position', Math.abs(undone.x - moved.x) < 0.1 && Math.abs(undone.y - moved.y) < 0.1, { moved, undone });
  await settled();
  const wander = await camera();
  p = await point('pixi_renamed'); await page.mouse.dblclick(p.x, p.y);
  await page.locator('[data-focused-asset="pixi_renamed"]:not([data-entering])').waitFor();
  check('double_click_enters_focus', new URL(page.url()).pathname.endsWith('/assets/pixi_renamed'));
  await page.keyboard.press('Escape');
  await page.locator('[data-focused-asset]').waitFor({ state: 'detached' }); await settled();
  const returned = await camera();
  check('focus_returns_wander_camera', Math.abs(wander.x - returned.x) < 0.5 && Math.abs(wander.y - returned.y) < 0.5 && Math.abs(wander.zoom - returned.zoom) < 0.0005, { wander, returned });
  await passive(); await canvas.focus(); await page.keyboard.press('ArrowRight');
  await until('keyboard selected', () => inspect('selectedNames'), names => names.join() === 'pixi_renamed');
  await page.keyboard.press('Enter');
  await page.locator('[data-focused-asset="pixi_renamed"]:not([data-entering])').waitFor();
  check('keyboard_enters_focus', true);
  await page.keyboard.press('Escape'); await page.locator('[data-focused-asset]').waitFor({ state: 'detached' }); await settled();
  await page.reload();
  await until('reload geometry', () => inspect('card', 'pixi_renamed'), Boolean); await settled();
  await picture('pixi_renamed', 'reload_real_pixels');
  const reloaded = await placed('pixi_renamed');
  check('reload_keeps_position', Math.abs(reloaded.x - moved.x) < 0.1 && Math.abs(reloaded.y - moved.y) < 0.1, reloaded);

  // Real pointer presses establish Pixi's internal drag state; blur is delivered explicitly in this headless browser.
  const blank = await openCanvas(page);
  await page.mouse.move(blank.x, blank.y); await page.mouse.down(); await page.mouse.move(blank.x - 40, blank.y + 20, { steps: 4 });
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  const canceledPan = await camera();
  await page.mouse.up(); await page.mouse.move(blank.x - 80, blank.y + 40, { steps: 4 }); await settled();
  check('blur_cancels_camera_drag', JSON.stringify(await camera()) === JSON.stringify(canceledPan));

  p = await point('pixi_renamed'); await page.mouse.move(p.x, p.y); await page.mouse.down();
  await page.mouse.move(p.x + 35, p.y + 25, { steps: 4 });
  const heldCamera = await camera();
  await page.mouse.wheel(20, 30);
  check('wheel_does_not_move_camera_during_card_drag', JSON.stringify(await camera()) === JSON.stringify(heldCamera));
  await page.keyboard.press('Escape'); await page.mouse.up(); await settled();
  await until('canceled card returns to persisted position', () => inspect('card', 'pixi_renamed'), card => card && Math.abs(card.position.x - moved.x) < .1 && Math.abs(card.position.y - moved.y) < .1);
  const afterCancel = await placed('pixi_renamed');
  check('escape_cancels_card_drag_without_write', Math.abs(afterCancel.x - moved.x) < .1 && Math.abs(afterCancel.y - moved.y) < .1);
  check('canceled_drag_clears_target_feedback', await page.locator('.stage[data-carrying],.stage[data-copying],.stage[data-offering]').count() === 0);

  // A hybrid mouse/touch session leaves a native overlay under the first finger. Exercise that actual missing-press path.
  const input = await context.newCDPSession(page);
  await input.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 2 });
  const touch = (type, points) => input.send('Input.dispatchTouchEvent', { type, touchPoints: points.map(({ x, y, id }) => ({ x, y, id, radiusX: 2, radiusY: 2, force: 1 })) });
  await passive(); await canvas.focus(); await page.keyboard.press('Escape');
  p = await point('pixi_renamed'); await page.mouse.move(p.x, p.y);
  await until('native overlay before touch pan', () => page.locator('.canvas-node').count(), count => count === 1);
  const beforeTouch = await camera();
  await touch('touchStart', [{ ...p, id: 1 }]);
  await touch('touchMove', [{ x: p.x + 40, y: p.y + 20, id: 1 }]);
  await touch('touchEnd', []); await settled();
  const afterTouch = await camera();
  check('native_overlay_touch_pans_canvas', Math.abs(afterTouch.x - beforeTouch.x) > 20 && Math.abs(afterTouch.zoom - beforeTouch.zoom) < .0005, { beforeTouch, afterTouch });
  p = await point('pixi_renamed'); await page.mouse.click(p.x, p.y);
  const beforePinch = await camera();
  await touch('touchStart', [{ x: p.x - 25, y: p.y, id: 2 }]);
  await touch('touchStart', [{ x: p.x - 25, y: p.y, id: 2 }, { x: p.x + 25, y: p.y, id: 3 }]);
  await touch('touchMove', [{ x: p.x - 30, y: p.y, id: 2 }, { x: p.x + 30, y: p.y, id: 3 }]);
  await touch('touchMove', [{ x: p.x - 55, y: p.y, id: 2 }, { x: p.x + 55, y: p.y, id: 3 }]);
  await touch('touchEnd', []); await settled();
  check('native_overlay_two_finger_pinch', (await camera()).zoom > beforePinch.zoom * 1.2, { before: beforePinch, after: await camera() });
  const afterPinch = await placed('pixi_renamed');
  check('pinch_does_not_commit_card_move', Math.abs(afterPinch.x - moved.x) < .1 && Math.abs(afterPinch.y - moved.y) < .1);
  await input.send('Emulation.setTouchEmulationEnabled', { enabled: false }); await input.detach();
  await passive(); await canvas.focus(); await page.keyboard.press('f'); await settled();
  // Exercise the actual WebGL loss/restore path without touching document state or another browser context.
  const beforeLoss = await inspect('card', 'pixi_renamed');
  const loss = await page.locator('canvas.canvas-pixels').evaluate(element => {
    const gl = element.getContext('webgl2') || element.getContext('webgl');
    const extension = gl?.getExtension('WEBGL_lose_context');
    if (!extension) return false;
    element.__restoreCanvas = () => extension.restoreContext();
    extension.loseContext(); return true;
  });
  check('context_loss_extension_available', loss);
  if (loss) {
    await until('actual graphics context lost', () => page.locator('canvas.canvas-pixels').evaluate(el => (el.getContext('webgl2') || el.getContext('webgl')).isContextLost()), Boolean);
    await page.locator('canvas.canvas-pixels').evaluate(el => { el.__restoreCanvas(); delete el.__restoreCanvas; });
    await until('actual graphics context restored', () => page.locator('canvas.canvas-pixels').evaluate(el => !(el.getContext('webgl2') || el.getContext('webgl')).isContextLost()), Boolean);
    await picture('pixi_renamed', 'context_restore_real_pixels');
    check('context_restore_keeps_geometry', JSON.stringify((await inspect('card', 'pixi_renamed')).bounds) === JSON.stringify(beforeLoss.bounds));
  }
  await passive(); await settled();
  await new Promise(done => setTimeout(done, 300));
  const idleBefore = await inspect('resources');
  await new Promise(done => setTimeout(done, 1500));
  const idleAfter = await inspect('resources');
  check('idle_canvas_stops_rendering', idleBefore.frames === idleAfter.frames && !idleAfter.scheduledFrame, { before: idleBefore.frames, after: idleAfter.frames, scheduled: idleAfter.scheduledFrame });

  during = 'real video import';
  await page.getByLabel('Add a file', { exact: true }).setInputFiles({ name: 'pixi_movie.mp4', mimeType: 'video/mp4', buffer: MP4 });
  await name.waitFor(); await name.fill('pixi_movie'); await name.press('Enter');
  await until('video import persisted', () => asset('pixi_movie'), value => value?.sha256 === `sha256:${digest(MP4)}` && value.duration_ms > 0);
  await fit(); await passive();
  p = await point('pixi_movie');
  await page.mouse.move(p.x, p.y);
  const preview = page.locator('.canvas-node[data-id] video.preview');
  await until('stationary mouse starts real video preview', () => preview.evaluate(video => !video.paused && video.currentTime > .1 && video.videoWidth > 0 && video.muted).catch(() => false), Boolean);
  check('first_stationary_hover_plays_video', true);
  const priorPreview = await preview.elementHandle();
  await passive();
  check('leaving_hover_releases_video_decoder', await priorPreview.evaluate(video => video.paused && !video.getAttribute('src') && video.readyState === 0));
  await priorPreview.dispose();
  const approvedBefore = await asset('pixi_movie'), approvalWrites = [];
  const approval = request => { if (request.method() === 'POST' && /\/(approve|unpin)$/.test(new URL(request.url()).pathname)) approvalWrites.push(new URL(request.url()).pathname); };
  page.on('request', approval);
  p = await point('pixi_movie'); await page.mouse.click(p.x, p.y); await page.keyboard.press('Enter');
  await page.locator('[data-focused-asset="pixi_movie"]:not([data-entering])').waitFor(); await settled();
  const approvedAfter = await asset('pixi_movie');
  check('enter_opens_current_mouse_selection', new URL(page.url()).pathname.endsWith('/assets/pixi_movie'));
  check('enter_does_not_also_toggle_video_approval', approvalWrites.length === 0 && approvedAfter.approved === approvedBefore.approved && approvedAfter.pinned === approvedBefore.pinned, { approvalWrites, before: { approved: approvedBefore.approved, pinned: approvedBefore.pinned }, after: { approved: approvedAfter.approved, pinned: approvedAfter.pinned } });
  page.off('request', approval);
  await page.keyboard.press('Escape'); await page.locator('[data-focused-asset]').waitFor({ state: 'detached' }); await passive(); await settled();
  check('all_api_requests_same_origin', apiOrigins.size === 1 && apiOrigins.has(origin), [...apiOrigins]);
  check('no_csp_violations', violations.length === 0, violations);
  check('no_page_errors', errors.length === 0, errors);
  seen.resources = await inspect('resources');
  sameService();
})().catch(async error => {
  failures.push(`${during}: ${clean(error.stack ?? error)}`);
  if (page) seen.failedResources = await page.locator('.canvas-root').evaluate(el => el.canvasInspection?.resources()).catch(() => null);
  if (page && out) await page.screenshot({ path: join(out, 'failure.png'), timeout: 5000 }).catch(() => {});
}).finally(async () => {
  if (browser) await browser.close().catch(() => {});
  const result = clean(JSON.stringify({ failures, seen, receipt: out }, null, 2));
  if (out) writeFileSync(join(out, 'result.json'), `${result}\n`);
  process.stdout.write(`${result}\n`);
  process.exitCode = failures.length ? 1 : 0;
});
