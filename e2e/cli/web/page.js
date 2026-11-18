// What the cells outside the web suite share (web.sh's web_alone: look, scale, pageidle, access; web_serves's serves.js; idle.sh's
// idle.js): a timeout on every wait, the page opened, and the project API. The suite's cells use fixtures.js.
const { execFileSync } = require('child_process');
const { existsSync, readFileSync, writeFileSync, readdirSync, readlinkSync, realpathSync } = require('fs');
const { homedir } = require('os');
const { join } = require('path');
const { DARK_OPEN, MP4, png } = require('./media');
const zlib = require('zlib');

const STEP_MS = 60_000;
// Past live.js's first run, the longest any cell waits between two checks.
const STALL_MS = 8 * 60_000;
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

// The camera still across three looks: a slow runner can draw one frame of a move in longer than a look. Missing geometry never settles.
async function settled(page) {
  let last, still = 0;
  await until('the canvas camera settled', () => page.evaluate(() => ({ view: document.querySelector('.canvas-root')?.canvasInspection?.camera(), moving: Boolean(document.querySelector('.stage[data-moving]')) })), ({ view, moving }) => {
    if (!view || moving) { still = 0; return false; }
    const now = JSON.stringify(view);
    still = now === last ? still + 1 : 0;
    last = now;
    return still >= 3;
  });
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
    let unread, entries;
    // A Calcifer holding protected projects is no dumpable process, its descriptors its own: an open project.db keeps its WAL.
    try { entries = readdirSync(fds); } catch (error) { if (error.code === 'EACCES') return existsSync(`${db}-wal`); throw error; }
    for (const fd of entries) {
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

// A mounted native surface only: Focus, or the sparse Wander card under the pointer. Use canvasBox for passive geometry/existence,
// and await hoverBox before reading a Wander card's controls. No hidden rich card tree stands in for the GPU scene.
const boxOf = (page, name) => page.locator(`[data-focused-asset=${JSON.stringify(name)}], .canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`).first();
async function closeBox(page) {
  // By the address, as the page has it: after a reload the box is on its way, and not there to count yet, and `page.url()` hears of a move a moment late.
  // Left already, by a click off it say, the canvas may still be on its way back.
  if (!(await page.evaluate(() => location.pathname.includes('/nodes/')))) return settled(page);
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
  // Wait for an editor (including non-image editors) or that actual next-take control.
  const editor = here.locator('.in-frame, .prominent, .tl, .media.blank:not(:has(.media-making, .made-from, .prog)), .media:has(.play, video, audio)');
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
  const card = page.locator(`[data-focused-asset=${JSON.stringify(name)}]:not([data-entering])`);
  if (await page.evaluate((asked) => location.pathname.endsWith(`/nodes/${encodeURIComponent(asked)}`), name)) {
    await card.waitFor();
    return settled(page);
  }
  await closeBox(page);
  const { x, y } = await reachBox(page, name);
  // A click only selects it: a double click opens it (Paul, 11-12).
  await page.mouse.dblclick(x, y);
  await card.waitFor();
  await settled(page);
}

// A point on open canvas, clear of every box and what floats over it: where a take dragged out lands as a box of its own.
const openCanvas = (page) => page.evaluate(() => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  if (!inspection) throw new Error('the canvas inspection is unavailable');
  for (let y = innerHeight - 60; y > 120; y -= 30) for (let x = innerWidth - 60; x > 320; x -= 30) {
    if (document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !inspection.hitTest({ x, y })) return { x, y };
  }
  throw new Error('no open canvas to drop on');
});

// The tab a proof is on, and what it looked like when the proof died: a thrown wait says only what it waited for.
let looking;
const lookAt = (page) => { looking = page; };
const boxLog = (page) => page.evaluate(() => ({
  log: window.boxLog?.slice(-40),
  focus: `${document.activeElement?.tagName}.${document.activeElement?.className}`,
  under: [...document.querySelectorAll(':hover')].at(-1)?.className,
  camera: document.querySelector('.canvas-root')?.canvasInspection?.camera(),
  selected: document.querySelector('.canvas-root')?.canvasInspection?.selectedNames(),
  resources: document.querySelector('.canvas-root')?.canvasInspection?.resources(),
}));
/** This computer's Calcifer, where its lock ($HOLDER) says: what serves each cell's page, unless $PAGE_ORIGIN does. */
const calcifer = () => process.env.PAGE_ORIGIN ?? `http://${JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).address}`;

/** The file at `path` held in `project`'s own content store, sent as the page sends a file (`PUT /v1/project-files`): its `sha256:`. */
async function upload(token, path, project) {
  if (!project) throw new Error(`upload ${path}: no project to hold it`);
  const bytes = readFileSync(path);
  const digest = `sha256:${require('crypto').createHash('sha256').update(bytes).digest('hex')}`;
  const at = `/v1/project-files/${digest}?project=${encodeURIComponent(project)}`;
  const answer = await fetch(`${calcifer()}${at}`, { method: 'PUT', headers: { Authorization: `Bearer ${token}` }, body: bytes });
  if (!answer.ok) throw new Error(`PUT ${at}: ${answer.status} ${await answer.text()}`);
  const held = await answer.json();
  if (held.digest !== digest || held.length !== bytes.length) throw new Error(`PUT ${at}: the reply names another file or length`);
  return digest;
}

/** Open the page at `to` (a URL on this Calcifer), else its home. The page holds no session (cl-980): Calcifer answers it as its own
 *  page, for the user it runs as, which this runner's is. */
async function signIn(page, to, options) {
  if (typeof to === 'object') [to, options] = [undefined, to];
  const link = to ? to.replace(/#.*$/, '') : `${calcifer()}/`;
  await require('./candidate').candidateUI(page, new URL(link).origin);
  return page.goto(link, options);
}

/** The bearer in Calcifer's lock record: what this computer's cozy, and a cell's own calls from outside the page, send. */
const holderToken = () => JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).token;

/** Chrome with its scrollbars shown, as on a person's screen, not hidden as headless has them. */
const BARS = { ignoreDefaultArgs: ['--hide-scrollbars'] };
/** This pass's browser: $ENGINE's own build (webkit, firefox), else the Chrome at $CHROME, launched with `options`. */
const launch = (playwright, options = {}) => (process.env.ENGINE ? playwright[process.env.ENGINE].launch({ headless: true }) : playwright.chromium.launch({ executablePath: process.env.CHROME, ...options, headless: true, ...(process.env.CHROME_ARGS ? { args: JSON.parse(process.env.CHROME_ARGS) } : {}) }));

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

const projectPath = (folder, suffix = '') => `/v1/projects/${encodeURIComponent(folder)}${suffix}`;
const projectAddresses = new Map();
async function projectRecord(reference) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const response = await fetch(`http://${holder.address}${projectPath(reference)}?assets=false`, { headers: { Authorization: `Bearer ${holder.token}` } });
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

/** Concurrent fixture mutations go directly to the existing project HTTP API. */
async function projectAction(folder, action, ...args) {
  const started = Date.now();
  const path = projectPath(folder);
  const request = async (method, suffix = '', body) => {
    const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
    const response = await fetch(`http://${holder.address}${path}${suffix}`, {
      method, headers: { Authorization: `Bearer ${holder.token}`, 'Content-Type': 'application/json' },
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

/** A new project's folder by its name, in this person's home folder: what the page's New takes, as `cozy new` does (cl-980). */
const projectFolder = (name) => join(homedir(), name);

/** A separate process lets synchronous fixture readers talk to the real Calcifer. */
const HTTP_READ = `
const fs = require('node:fs');
const http = require('node:http');
const holder = JSON.parse(fs.readFileSync(process.env.HOLDER, 'utf8'));
const body = fs.readFileSync(0);
const url = new URL(process.argv[2], 'http://' + holder.address);
const request = http.request(url, { method: process.argv[1], headers: {
  Authorization: 'Bearer ' + holder.token,
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

function projectJson(folder, method = 'GET', suffix = '', body) {
  const text = apiText(method, projectPath(folder, suffix), body === undefined ? '' : JSON.stringify(body));
  return text ? JSON.parse(text) : null;
}

/** Import a real file by the same content endpoint the browser uses. */
function importFile(folder, file, name) {
  const bytes = readFileSync(file);
  const sha256 = `sha256:${require('crypto').createHash('sha256').update(bytes).digest('hex')}`;
  apiText('PUT', `/v1/project-files/${sha256}?project=${encodeURIComponent(folder)}`, bytes, 'application/octet-stream');
  const stem = require('path').parse(file).name;
  return projectJson(folder, 'POST', '/imports', { source: sha256, name: name ?? stem, exact: name !== undefined });
}

/** Create starter-film fixtures through the project API. */
async function newProject(folder) {
  const holder = JSON.parse(readFileSync(process.env.HOLDER, 'utf8'));
  const answer = await fetch(`http://${holder.address}/v1/projects`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${holder.token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ path: folder, starter: true, unencrypted: true }),
  });
  const opened = await answer.json();
  if (!answer.ok) throw new Error(`create starter project ${folder}: ${answer.status} ${JSON.stringify(opened)}`);
  return opened;
}

/** Decode Chrome's 8-bit RGB/RGBA screenshot in Node, independently of browser Canvas2D/GPU readback. */
function screenshotPixels(bytes) {
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
  const raw = zlib.inflateSync(Buffer.concat(chunks)), stride = width * channels, decoded = Buffer.alloc(stride * height);
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
  const read = (x, y) => {
    x = Math.round(x); y = Math.round(y);
    if (x < 0 || x >= width || y < 0 || y >= height) throw new Error('pixel probe is offscreen');
    return [...decoded.subarray((y * width + x) * channels, (y * width + x) * channels + 3)];
  };
  read.width = width; read.height = height;
  return read;
}

/** Read-only product geometry, never evidence that the renderer presented its pixels. Null while the surface is unavailable. */
const canvasView = (page) => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.camera() ?? null);
const canvasBox = (page, name) => page.evaluate((name) => document.querySelector('.canvas-root')?.canvasInspection?.card(name) ?? null, name);
const canvasNames = (page) => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.names() ?? null);
const selectedBoxes = (page) => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.selectedNames() ?? null);

/** A body point that is actually reachable: native popovers and another card can cover otherwise valid geometry. */
const reachableBox = (page, name) => page.evaluate((name) => {
  const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
  const box = inspection?.card(name)?.body;
  if (!box || box.width <= 0 || box.height <= 0) return null;
  for (const [x, y] of [[0.5, 0.5], [0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]]) {
    const point = { x: box.x + box.width * x, y: box.y + box.height * y };
    const target = document.elementFromPoint(point.x, point.y);
    if (target && root.contains(target) && !target.closest('button, input, textarea, select, [contenteditable]') && inspection.hitTest(point)?.name === name) return point;
  }
  return null;
}, name);

async function reachBox(page, name) {
  await until(`${name} has canvas geometry`, () => canvasBox(page, name), Boolean);
  if (!(await reachableBox(page, name))) await fitCanvas(page);
  return until(`${name} in reach`, () => reachableBox(page, name), Boolean);
}

/** Mount the real native card by hovering it. No inspection method changes selection, the camera, or the document. */
async function hoverBox(page, name) {
  const focused = page.locator(`[data-focused-asset=${JSON.stringify(name)}]`);
  if (await focused.count()) { await focused.waitFor(); return boxOf(page, name); }
  await closeBox(page);
  const point = await reachBox(page, name);
  await page.mouse.move(point.x, point.y);
  const native = boxOf(page, name);
  await native.waitFor();
  return native;
}

module.exports = { holderToken, projectFolder, projectUrl, projectFromPage, projectAction, projectJson, importFile, newProject, BARS, DARK_OPEN, MP4, STEP_MS, boxOf, calcifer, closeBox, openBox, openCanvas, fitCanvas, lastLook, launch, lookAt, noStall, png, projectHeld, renderOf, settled, signIn, until, upload, screenshotPixels, canvasView, canvasBox, canvasNames, selectedBoxes, hoverBox };
