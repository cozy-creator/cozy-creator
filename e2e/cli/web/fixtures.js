// The web suite's fixtures (cl-973): each cell on a project and a browser context of its own, on this job's Calcifer, several at once.
// Guards fail a cell on a page error, a console error, a CSP violation or a modal. Waits are on state; `observeStill` and `stays` are the
// windows.
//   const { test, expect, box, ... } = require('./fixtures')
// $HOLDER: Calcifer's lock record; $COZY: the CLI; $OUT: where the job keeps what it made; $ARGV_LOG: each cozy call, for coverage.
const { test: base, expect } = require('@playwright/test');
const { execFile } = require('child_process');
const { createHash, randomBytes } = require('crypto');
const { appendFileSync, readFileSync, rmSync, writeFileSync } = require('fs');
const { homedir, loadavg } = require('os');
const { basename, join } = require('path');
const { DARK_OPEN, MP4, photo, png, wav } = require('./media');
const { screenshotPixels } = require('./page');

const STORAGE = { 'cozy-project-storage': '3' };
// What a cell may remove: only what this suite made.
const MADE = 'web-';

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: web.sh's web_suite sets it`); })();
const slug = (title) => title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 24);

/** Calcifer's HTTP API as the holder (its lock record's bearer), on project storage protocol 3. */
class Api {
  constructor(origin, token) {
    this.origin = origin;
    this.token = token;
  }

  /** The answer as it came, whatever its status. A body that isn't bytes or a string goes as JSON. */
  raw(method, path, { body, type = 'application/json', headers = {} } = {}) {
    const sent = body === undefined || typeof body === 'string' || body instanceof Uint8Array ? body : JSON.stringify(body);
    return fetch(this.origin + path, {
      method,
      headers: { Authorization: `Bearer ${this.token}`, ...STORAGE, ...(sent === undefined ? {} : { 'Content-Type': type }), ...headers },
      body: sent,
      signal: AbortSignal.timeout(60_000),
    });
  }

  /** Its JSON, null when empty. Any status but 2xx throws, with what Calcifer said. */
  async call(method, path, body, options) {
    const answer = await this.raw(method, path, { body, ...options });
    const text = await answer.text();
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${text.slice(0, 500)}`);
    return text ? JSON.parse(text) : null;
  }

  get(path) { return this.call('GET', path); }
  post(path, body = {}) { return this.call('POST', path, body); }
  patch(path, body) { return this.call('PATCH', path, body); }
  delete(path) { return this.call('DELETE', path); }
}

/** A cell's own project, made through the API in a folder of its own. Only project-scoped helpers: a shared cell edits nothing else. */
class Project {
  constructor(api, opened, passphrase) {
    Object.assign(this, { api, id: opened.id, code: opened.code, path: opened.path, name: opened.name, passphrase, recovery: opened.recovery_secret });
  }

  /** Its page, `/p/<code>`, or what's under it: `assets/A`, `projects`. */
  url(rest = '') { return `${this.api.origin}/p/${this.code}${rest ? `/${rest}` : ''}`; }
  /** Its API path, or what's under it: `/assets`, `/history`. */
  at(rest = '') { return `/v1/projects/${encodeURIComponent(this.id)}${rest}`; }
  state() { return this.api.get(this.at()); }
  async assets() { return (await this.state()).assets; }
  async asset(name) { return (await this.assets()).find((row) => row.name === name); }
  /** A box exactly named, as the page posts one: `{name, tool, kind, fields, at}`. A box with no tool needs `fields.text` or a `kind`. */
  add(box) { return this.api.post(this.at('/assets'), { tool: '', exact: true, ...box }); }

  /** Bytes held in its own content store, as the page sends a file: their `sha256:`. */
  async upload(bytes) {
    const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
    const held = await this.api.call('PUT', `/v1/project-files/${digest}?project=${encodeURIComponent(this.id)}`, bytes, { type: 'application/octet-stream' });
    if (held.digest !== digest || held.length !== bytes.length) throw new Error(`PUT ${digest}: held ${held.digest}, ${held.length} bytes`);
    return digest;
  }

  /** A file as a box called `name`, at `at` ({x, y}) when given. */
  async import(bytes, { name, at, ...rest }) {
    return this.api.post(this.at('/imports'), { source: await this.upload(bytes), name, exact: true, ...(at ? { at } : {}), ...rest });
  }
}

/** Calcifer's lock record, or null while there's none to read: still opening its store, or being written. */
function holderNow() {
  try {
    return JSON.parse(readFileSync(need('HOLDER'), 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT' || error instanceof SyntaxError) return null;
    throw error;
  }
}
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch (error) { return error.code === 'EPERM'; } };
/** Whether `holder`'s Calcifer answers its status, as its bearer. */
const answers = (holder) => fetch(`http://${holder.address}/v1/status`, { headers: { Authorization: `Bearer ${holder.token}`, ...STORAGE }, signal: AbortSignal.timeout(2_000) })
  .then((answer) => answer.ok, () => false);
/** Whether anything listens at `address`. */
const listens = (address) => fetch(`http://${address}/v1/status`, { signal: AbortSignal.timeout(2_000) }).then(() => true, () => false);

/** This job's Calcifer itself, for a solo cell: stopped as its service manager would leave it, started as the CLI starts it, killed, put
 *  on another Hub. Each new one is read from its lock record into `calcifer` and `api`, in place, for every fixture holding them. */
class CalciferControl {
  constructor(calcifer, api) {
    this.calcifer = calcifer;
    this.api = api;
    this.holder = holderNow();
    this.config = join(homedir(), '.cozy', 'config.toml');
    this.kept = readFileSync(this.config, 'utf8');
    this.suite = need('COZY_HUB_URL');
    this.on = this.suite;
    // This computer's machine (TensorD, `cozy machine serve`): its lock record says its pid.
    this.machine = join(homedir(), '.cozy', 'machine', 'var', 'lib', 'cozy', 'machine', 'agent.lock.holder');
  }

  /** SIGTERM, a clean exit, which systemd's on-failure and launchd's KeepAlive leave down: once it's gone and nothing listens. */
  async stop() {
    const { pid, address } = this.holder;
    process.kill(pid, 'SIGTERM');
    await expect.poll(async () => alive(pid) || listens(address), { message: `Calcifer ${pid} stopped`, timeout: 60_000 }).toBe(false);
  }

  /** `cozy machine ls`, on the Hub it's on now, starts it: once a new one answers. */
  async start() {
    const was = this.holder.pid;
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, 'machine\x1fls\x1f\x1e');
    const said = await new Promise((done) => execFile(need('COZY'), ['machine', 'ls'], { env: { ...process.env, COZY_HUB_URL: this.on }, timeout: 300_000 },
      (error, stdout, stderr) => done(`exit ${error ? error.code ?? 1 : 0}: ${`${stdout}${stderr}`.slice(-400)}`)));
    await this.back(was, `cozy machine ls started it (${said})`);
  }

  /** SIGKILL, sent as it's called: its service manager starts it again (Windows has none, so the CLI does). Once a new one answers. */
  async kill() {
    const was = this.holder.pid;
    process.kill(was, 'SIGKILL');
    if (process.platform !== 'win32') return this.back(was, 'its service manager started it again');
    await expect.poll(() => alive(was), { message: `Calcifer ${was} killed`, timeout: 60_000 }).toBe(false);
    return this.start();
  }

  /** On `url`'s Hub, else the suite's: config.toml's top-level `hub_url` and its origin among `local_origins` (first, as after a [table]
   *  they would be that table's), Calcifer and this computer's machine restarted. A machine declares its local origins once, as it
   *  starts (xs-196 D), and refuses a run naming a loopback Hub it wasn't told of: `invalid_hub_origin`. */
  async hub(url = this.suite) {
    const lines = readFileSync(this.config, 'utf8').split('\n');
    const declared = lines.find((line) => /^local_origins\s*=/.test(line))?.replace(/^local_origins\s*=\s*"?|"?\s*$/g, '') ?? '';
    const rest = lines.filter((line) => !/^(hub_url|local_origins)\s*=/.test(line));
    const origins = [new URL(url).origin, ...declared.split(',').filter(Boolean)].join(',');
    writeFileSync(this.config, url === this.suite ? this.kept : [`hub_url = "${url}"`, `local_origins = "${origins}"`, ...rest].join('\n'));
    this.on = url;
    await this.stop();
    await this.stopMachine();
    await this.start();
    await this.startMachine();
  }

  /** This computer's machine stopped (SIGTERM): it reads config.toml again as Calcifer next starts it. None running: nothing to do. */
  async stopMachine() {
    let pid;
    try { pid = JSON.parse(readFileSync(this.machine, 'utf8')).pid; } catch (error) { if (error.code === 'ENOENT') return; throw error; }
    if (!pid || !alive(pid)) return;
    process.kill(pid, 'SIGTERM');
    await expect.poll(() => alive(pid), { message: `this computer's machine (pid ${pid}) stopped`, timeout: 60_000 }).toBe(false);
  }

  /** `cozy machine show local` starts this computer's machine (e2e/lib.sh's machine_ready): once it says it's ready. */
  async startMachine() {
    await expect.poll(() => new Promise((done) => {
      if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, '--json\x1fmachine\x1fshow\x1flocal\x1f\x1e');
      execFile(need('COZY'), ['--json', 'machine', 'show', 'local'], { timeout: 120_000 }, (error, stdout) => {
        try { done(JSON.parse(stdout).state ?? `exit ${error?.code ?? 0}`); } catch { done(`exit ${error?.code ?? 0}: ${String(stdout).slice(-200)}`); }
      });
    }), { message: "this computer's machine ready", timeout: 180_000, intervals: [2_000, 5_000] }).toBe('ready');
  }

  /** A new Calcifer than `was`, answering: its lock record read into `calcifer` and `api`. */
  async back(was, how) {
    let now = null;
    await expect.poll(async () => {
      const read = holderNow();
      now = read && read.pid !== was && alive(read.pid) && (await answers(read)) ? read : null;
      return Boolean(now);
    }, { message: `a new Calcifer after ${was} answering: ${how}`, timeout: 120_000 }).toBe(true);
    this.holder = now;
    Object.assign(this.calcifer, { origin: `http://${now.address}`, token: now.token, pid: now.pid });
    Object.assign(this.api, { origin: this.calcifer.origin, token: now.token });
  }

  /** As it was: running, on the suite's Hub, config.toml as it was found. */
  async restore() {
    const moved = this.on !== this.suite || readFileSync(this.config, 'utf8') !== this.kept;
    const read = holderNow();
    const up = Boolean(read && alive(read.pid) && (await answers(read)));
    if (read) this.holder = read;
    if (moved) {
      writeFileSync(this.config, this.kept);
      this.on = this.suite;
      if (up) await this.stop();
      await this.stopMachine();
    }
    if (moved || !up) await this.start();
    if (moved) await this.startMachine();
  }
}

/** A folder this suite made, once nothing holds it: Windows refuses one Calcifer still reads. */
async function removeFolder(folder) {
  if (!basename(folder).startsWith(MADE)) throw new Error(`refusing to remove ${folder}: the web suite didn't make it`);
  await expect.poll(() => {
    try {
      rmSync(folder, { recursive: true, force: true });
      return 'removed';
    } catch (error) {
      // ENOTEMPTY: Calcifer still finishing a write in it (a run just canceled).
      if (error.code !== 'EPERM' && error.code !== 'EBUSY' && error.code !== 'ENOTEMPTY') throw error;
      return error.code;
    }
  }, { message: `${folder} removed`, timeout: 60_000 }).toBe('removed');
}

/** In every page of a guarded context: CSP violations and modal dialogs, said to the fixture as they happen. */
/** Test-only, in this file and nowhere in the product (no setting or URL reaches it; coordinator, 11-16): a runner has no GPU, and the canvas
 *  draws in 2D when WebGL is only software (cl-1048). Every lane but web-2d and web-2d-solo names the runner's software WebGL as a hardware GPU, so the
 *  canvas takes the WebGL path most people's computers take, still drawn by the runner's software GL; those two (WEB_RENDERER=2d, run.sh)
 *  leave the browser as it is, the path of a computer with no GPU. */
function hardwareGl() {
  for (const Context of [globalThis.WebGLRenderingContext, globalThis.WebGL2RenderingContext].filter(Boolean)) {
    const read = Context.prototype.getParameter;
    Context.prototype.getParameter = function (name) {
      // RENDERER, and WEBGL_debug_renderer_info's UNMASKED_RENDERER_WEBGL.
      if (name === 0x1f01 || name === 0x9246) return 'Test GPU (hardware, for the web suite)';
      return read.call(this, name);
    };
  }
}
const WEBGL_PATH = process.env.WEB_RENDERER !== '2d';

function guardPage() {
  addEventListener('securitypolicyviolation', (event) => window.__cozyGuard(`CSP violation: ${event.violatedDirective} blocked ${event.blockedURI}`));
  const showModal = HTMLDialogElement.prototype.showModal;
  HTMLDialogElement.prototype.showModal = function () {
    window.__cozyGuard(`a modal over the page: ${this.getAttribute('aria-label') ?? this.className}`);
    return showModal.call(this);
  };
}

/** What opens and closes boxes, in order: the pointer, presses, focus, boxes and the canvas moving, boxes coming and going. Attached on failure. */
function watchBoxes() {
  const log = (window.boxLog = []);
  const say = (line) => {
    log.push(`${Math.round(performance.now())} ${line}`);
    if (log.length > 300) log.shift();
  };
  const boxOf = (el) => el?.closest?.('.canvas-node')?.querySelector('.card')?.dataset.asset ?? el?.closest?.('[data-focused-asset]')?.dataset.focusedAsset ?? null;
  let under = null;
  document.addEventListener('pointerover', ({ target, clientX, clientY }) => {
    const now = boxOf(target) ?? document.querySelector('.canvas-root')?.canvasInspection?.hitTest({ x: clientX, y: clientY })?.name ?? null;
    if (now !== under) say(`pointer ${under} -> ${now} at ${clientX},${clientY}`);
    under = now;
  }, true);
  for (const type of ['mousedown', 'mouseup', 'click', 'dblclick', 'keydown', 'pointercancel', 'dragstart']) {
    document.addEventListener(type, ({ target, key }) => say(`${type}${key ? ` ${key}` : ''} ${target.tagName}.${target.className}`), true);
  }
  document.addEventListener('focusin', ({ target }) => say(`focus ${boxOf(target)} ${target.tagName}.${target.className}`), true);
  new MutationObserver((records) => {
    for (const { type, target, attributeName, oldValue, addedNodes, removedNodes } of records) {
      for (const node of addedNodes) if (node.classList?.contains('canvas-node')) say(`native mounted ${boxOf(node)} ${node.style.left},${node.style.top}`);
      for (const node of removedNodes) if (node.classList?.contains('canvas-node')) say(`native unmounted ${node.dataset.id}`);
      if (type !== 'attributes') continue;
      if (attributeName === 'data-moving') say(`canvas ${target.hasAttribute('data-moving') ? 'moving' : 'still'}`);
      if (attributeName === 'aria-selected') say(`${target.dataset.asset} selected=${target.getAttribute('aria-selected')}`);
      if (attributeName === 'style' && !oldValue?.includes(target.style.transform)) {
        if (target.classList.contains('canvas-world')) say(`canvas at ${target.style.transform}`);
        if (target.classList.contains('canvas-node')) say(`${boxOf(target)} native at ${target.style.left},${target.style.top}`);
      }
    }
  }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['style', 'data-moving', 'aria-selected'], attributeOldValue: true });
}

/** WebKit's words for a page's fetch that failed, its reason masked: the URL it asked. */
const CUT_FETCH = /^Fetch API cannot load (\S+) due to access control checks\.$/;

const test = base.extend({
  /** This job's Calcifer, from its lock record ($HOLDER), read once a worker. */
  calcifer: [async ({}, use) => {
    const holder = JSON.parse(readFileSync(need('HOLDER'), 'utf8'));
    await use({ origin: `http://${holder.address}`, token: holder.token, pid: holder.pid });
  }, { scope: 'worker' }],

  /** Folders a cell made, removed after everything else is torn down (an auto fixture is set up first and torn down last); and the
   *  1-minute load as the cell ended, in the report. */
  leftovers: [async ({}, use, testInfo) => {
    const folders = [];
    await use(folders);
    testInfo.annotations.push({ type: 'load', description: loadavg()[0].toFixed(2) });
    for (const folder of folders) await removeFolder(folder);
  }, { auto: true }],

  /** A folder the cell made some other way (a project made from the page), removed with its own. */
  removeAfter: async ({ leftovers }, use) => use((folder) => { leftovers.push(folder); }),

  api: async ({ calcifer }, use) => use(new Api(calcifer.origin, calcifer.token)),

  /** Calcifer stopped, started, killed or put on another Hub (CalciferControl): the solo project's alone, as every other cell shares this
   *  Calcifer. Put back as it was after the cell, however it ended. */
  calciferControl: async ({ calcifer, api, guards }, use, testInfo) => {
    if (testInfo.project.name !== 'solo') throw new Error(`calciferControl is the solo project's: ${testInfo.project.name}'s cells share this Calcifer`);
    const control = new CalciferControl(calcifer, api);
    await use(control);
    // Put back with the cell over: what its pages say as Calcifer restarts is this teardown's doing.
    guards.over();
    await control.restore();
  },

  /** `test.use({ projectOptions: { starter: true } })`: the starter film; `encrypted: true`: a vault, its passphrase on `project`. */
  projectOptions: [{ starter: false, encrypted: false }, { option: true }],

  project: async ({ api, projectOptions, leftovers }, use, testInfo) => {
    const folder = join(need('OUT'), 'web', 'projects', `${MADE}${slug(testInfo.title)}-${randomBytes(4).toString('hex')}`);
    leftovers.push(folder);
    const passphrase = projectOptions.encrypted ? randomBytes(18).toString('base64url') : undefined;
    const opened = await api.post('/v1/projects', { path: folder, starter: projectOptions.starter, ...(passphrase ? { passphrase } : { unencrypted: true }) });
    await use(new Project(api, opened, passphrase));
  },

  /** Fails the cell on what any of its pages said: a page error, a console error, a CSP violation, a modal. `allow(re)` takes what a cell
   *  brings about on purpose, matched against `<message> <url>`. */
  guards: async ({}, use) => {
    const said = [];
    const allowed = [];
    let over = false;
    const say = (line) => { if (!over && !allowed.some((re) => re.test(line))) said.push(line); };
    await use({
      allow: (...patterns) => { allowed.push(...patterns); },
      /** The cell is over: what its pages say from here is Playwright's doing as it closes them (WebKit's screenshot adds a
       *  `<style>body {}</style>`, which the page's CSP refuses), not the page's. */
      over: () => { over = true; },
      async watch(context) {
        await context.exposeBinding('__cozyGuard', (_, line) => say(line));
        await context.addInitScript(guardPage);
        const watch = (page) => {
          // From a main-frame navigation until the next document's DOMContentLoaded, the last document is ending. WebKit says a fetch of
          // its own Calcifer's /v1 cut as it ends (and caught by the page: its event stream, a read in flight) as a page error in its masked
          // words: only that is the document's end. Anywhere else in a document's life it fails the cell (run 43730578847: `open`'s goto
          // from a project to /).
          let ending = null;
          page.on('request', (request) => {
            if (!request.isNavigationRequest() || request.frame() !== page.mainFrame()) return;
            const was = new URL(page.url());
            ending = /^https?:$/.test(was.protocol) ? was.origin : null;
          });
          page.on('domcontentloaded', () => { ending = null; });
          const cut = (error) => {
            const asked = (error.stack ?? '').split('\n')[0].match(CUT_FETCH)?.[1];
            if (!ending || !asked || !error.message.endsWith(' due to access control checks.')) return false;
            const url = new URL(asked);
            return url.origin === ending && url.pathname.startsWith('/v1/');
          };
          page.on('pageerror', (error) => {
            if (cut(error)) return;
            say(`page error: ${error.message}`);
          });
          page.on('console', (message) => { if (message.type() === 'error') say(`console error: ${message.text()} ${message.location().url}`); });
        };
        context.pages().forEach(watch);
        context.on('page', watch);
      },
    });
    expect(said, 'page errors, console errors, CSP violations and modals').toEqual([]);
  },

  context: async ({ context, guards }, use) => {
    await guards.watch(context);
    if (WEBGL_PATH) await context.addInitScript(hardwareGl);
    await context.addInitScript(watchBoxes);
    await use(context);
    guards.over();
  },

  /** Another browser context, as another person's browser: guarded, closed after the cell. */
  newContext: async ({ browser, guards }, use) => {
    const made = [];
    await use(async (options = {}) => {
      const context = await browser.newContext(options);
      made.push(context);
      await guards.watch(context);
      if (WEBGL_PATH) await context.addInitScript(hardwareGl);
      return context;
    });
    guards.over();
    for (const context of made) await context.close();
  },

  /** Optional frozen candidate delivery. Chromium keeps normal HTTP caching; the other engines use an explicit functional-only route. */
  candidate: async ({ browserName }, use, testInfo) => {
    const receipts = new Set();
    const fallbackPages = new WeakSet();
    await use(async (page, origin) => {
      if (!process.env.CANDIDATE_DIST) return;
      if (browserName === 'chromium') {
        const receipt = await require('./candidate-cdp').candidateCDP(page, origin, process.env.CANDIDATE_DIST);
        receipts.add(receipt);
      } else {
        if (/^opens(?:\s|$)/.test(testInfo.title)) throw new Error('candidate open timings require Chromium CDP delivery with normal HTTP caching');
        if (!fallbackPages.has(page)) {
          testInfo.annotations.push({ type: 'candidate', description: `${browserName}: functional-only Playwright routing; HTTP caching disabled` });
          fallbackPages.add(page);
        }
        await require('./candidate').candidateUI(page, origin);
      }
    });
    for (const receipt of receipts) {
      await receipt.dispose();
      expect.soft(receipt.errors, 'candidate delivery errors').toEqual([]);
      expect.soft(receipt.documents.some(document => document.replaced), 'the candidate replaced a real Calcifer HTML/CSP response').toBe(true);
    }
    if (receipts.size) await testInfo.attach('candidate-delivery', { body: Buffer.from(JSON.stringify([...receipts], null, 2)), contentType: 'application/json' });
  },

  /** Open Calcifer's sessionless page as its own local user, preserving real origin/auth/CSP and optional candidate delivery. */
  signIn: async ({ candidate }, use) => {
    await use(async (page, to) => {
      await candidate(page, new URL(to).origin);
      return page.goto(to.replace(/#.*$/, ''));
    });
  },

  /** A tab open at the cell's project, its canvas up. The box log is attached when the cell fails. */
  page: async ({ page, project, signIn, guards }, use, testInfo) => {
    await signIn(page, project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await use(page);
    // Torn down before the context, which closes this page: the cell is over from here.
    guards.over();
    // Which renderer drew the cell, and how many frames it drew in how long: read on every runner, kept with its result.
    const drew = page.isClosed() ? null : await page.evaluate(() => {
      const resources = document.querySelector('.canvas-root')?.canvasInspection?.resources();
      return resources ? { graphics: resources.graphics, frames: resources.frames, ms: Math.round(performance.now()) } : null;
    }).catch(() => null);
    if (drew) testInfo.annotations.push({ type: 'renderer', description: JSON.stringify(drew) });
    if (testInfo.status !== testInfo.expectedStatus && !page.isClosed()) {
      const said = await page.evaluate(() => ({ url: location.href, log: window.boxLog?.slice(-60), focus: `${document.activeElement?.tagName}.${document.activeElement?.className}` }));
      await testInfo.attach('boxes', { body: JSON.stringify(said, null, 2), contentType: 'application/json' });
    }
  },

  /** `cozy <words>` in the project's folder, as a person's shell runs it: {code, stdout, stderr}. `cli.json(...)` parses `--json`'s, or throws. */
  cli: async ({ project }, use) => {
    const cozy = (...words) => new Promise((done) => {
      if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
      execFile(need('COZY'), words, { cwd: project.path, maxBuffer: 1 << 26, timeout: 300_000 }, (error, stdout, stderr) => done({ code: error ? error.code ?? 1 : 0, stdout, stderr }));
    });
    cozy.json = async (...words) => {
      const said = await cozy('--json', ...words);
      if (said.code !== 0) throw new Error(`cozy --json ${words.join(' ')}: exit ${said.code}: ${said.stderr.slice(-500)}`);
      return JSON.parse(said.stdout);
    };
    await use(cozy);
  },
});

/** A mounted native card: Focus or the one under the pointer. Passive cards use canvasCard; controls require hoverBox. */
const box = (page, name) => page.locator(`.canvas-node .card[data-asset=${JSON.stringify(name)}], [data-focused-asset=${JSON.stringify(name)}] .card[data-asset=${JSON.stringify(name)}]`).first();

/** Read-only product geometry and logical names. Neither is evidence that the GPU presented the expected pixels.
 *  A file still on its way to Calcifer is drawn under the name it was given while the scene and its card key it `placing:N` (place.ts):
 *  these helpers take and give the name a person sees, from its drawn title or, mounted under the pointer, its card's. */
const SHOWN = `(inspection, key) => {
  if (!key.startsWith('placing:')) return key;
  const title = inspection.card(key)?.rendered?.labels?.title?.title;
  if (title) return title;
  // Mounted under the pointer, its card says the name it's drawn under (Card.tsx data-asset is the view's name).
  const native = inspection.interaction().nativeHover === key ? document.querySelector('.canvas-node .card[data-asset]') : null;
  return native?.dataset.asset || key;
}`;
const KEY = `(inspection, name) => inspection.card(name) ? name : inspection.names().find((key) => key.startsWith('placing:') && (${SHOWN})(inspection, key) === name)`;
const canvasCard = (page, name) => page.evaluate(`((name) => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  const key = inspection && (${KEY})(inspection, name);
  return key ? inspection.card(key) : null;
})(${JSON.stringify(name)})`);
const canvasNames = (page) => page.evaluate(`(() => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  return inspection?.names().map((key) => (${SHOWN})(inspection, key)) ?? [];
})()`);
const selectedNames = (page) => page.evaluate(() => [...(document.querySelector('.canvas-root')?.canvasInspection?.selectedNames() ?? [])].sort());
const canvasVisible = (page, name) => page.evaluate(`((name) => {
  const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
  const key = inspection && (${KEY})(inspection, name);
  const card = key && inspection.card(key);
  const native = key && document.querySelector('.canvas-node .card[data-asset=' + JSON.stringify(name) + ']');
  if (!root || !card || (!card.rendered && !native?.checkVisibility())) return false;
  const r = native?.getBoundingClientRect() ?? card.bounds, view = root.getBoundingClientRect();
  return r.width > 0 && r.height > 0 && r.x + r.width > view.left && r.y + r.height > view.top && r.x < view.right && r.y < view.bottom;
})(${JSON.stringify(name)})`);

/** Text held by actual rendered labels/words, or the committed native card; never read from the document store. */
const canvasText = (page, name) => page.evaluate((name) => {
  const native = document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}], [data-focused-asset=${JSON.stringify(name)}] .card[data-asset=${JSON.stringify(name)}]`);
  if (native) return native.textContent;
  const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered;
  return shown ? [shown.words, ...Object.values(shown.labels).map(label => label.fullText)].filter(Boolean).join(' ') : null;
}, name);

const canvasLabel = (page, name, key) => page.evaluate(({ name, key }) => {
  const shown = document.querySelector('.canvas-root')?.canvasInspection?.card(name)?.rendered;
  if (shown) return shown.labels[key]?.fullText ?? null;
  const native = document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}], [data-focused-asset=${JSON.stringify(name)}] .card[data-asset=${JSON.stringify(name)}]`);
  const selector = { title: '.head .nm', version: '.node-version-label', tool: '.node-tool', shape: '.ratio-rest', progress: '.prog-state' }[key];
  return selector ? native?.querySelector(selector)?.textContent ?? null : null;
}, { name, key });

/** The middle of a rectangle, or a real native control. No locator click scrolls the canvas from under the pointer. */
async function middle(subject) {
  const { x, y, width, height } = typeof subject.boundingBox === 'function' ? await subject.boundingBox() : await subject;
  return { x: x + width / 2, y: y + height / 2 };
}

/** A reachable body/title point from the current renderer geometry, after layout is available. */
async function canvasPoint(page, name, part = 'body') {
  let point;
  await expect.poll(async () => {
    point = await page.evaluate(({ name, part }) => {
      const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection, card = inspection?.card(name);
      const nativeTitle = part === 'title' && document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}] .head .nm`);
      // The new canvas (cl-1048) says where each of a card's parts is drawn; the old one, its title label and its body.
      const rect = card?.rendered?.parts?.[part] ?? (part === 'title' ? nativeTitle?.getBoundingClientRect() ?? card?.rendered?.labels.title ?? card?.title : card?.[part]);
      if (!rect || !rect.width || !rect.height) return null;
      const point = { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
      const target = document.elementFromPoint(point.x, point.y);
      return target && root.contains(target) && inspection.hitTest(point)?.name === name ? point : null;
    }, { name, part });
    return Boolean(point);
  }, { message: `${name}'s ${part} is reachable` }).toBe(true);
  return point;
}

/** Whether the page draws the new canvas (cl-1048): every card drawn by the scene, none mounted as a DOM card under the pointer. */
const newCanvas = (page) => page.evaluate(() => typeof document.querySelector('.canvas-root')?.canvasInspection?.cards === 'function');

/** The pointer on a box. The old canvas mounts its DOM card there (returned); the new draws it hovered, and has no DOM card: act on it with
 *  partPoint, renameField, cornerPoint and previewState. An existing Focus stays open so another card can be carried into it. */
async function hoverBox(page, name) {
  const focused = page.locator(`[data-focused-asset=${JSON.stringify(name)}]`);
  if (await focused.count()) return box(page, name);
  const point = await canvasPoint(page, name);
  await page.mouse.move(point.x, point.y);
  if (await newCanvas(page)) {
    await expect.poll(() => page.evaluate((name) => {
      const card = document.querySelector('.canvas-root').canvasInspection.cards().find((one) => one.name === name);
      return Boolean(card) && card.hovered !== false;
    }, name), { message: `${name} hovered` }).toBe(true);
    return box(page, name);
  }
  await expect(box(page, name)).toBeVisible();
  return box(page, name);
}

/** Where a box's drawn part is on the screen (body, title, delete, cancel, corner), read with nothing under the pointer. */
async function partPoint(page, name, part) {
  const away = await openPoint(page);
  await page.mouse.move(away.x, away.y);
  let point = null;
  await expect.poll(async () => {
    point = await page.evaluate(({ name, part }) => {
      const card = document.querySelector('.canvas-root')?.canvasInspection?.card(name), shown = card?.rendered;
      const rect = shown?.parts?.[part]
        ?? (part === 'delete' || part === 'cancel' ? shown?.controls?.find((one) => one.part === part) : part === 'title' ? shown?.labels?.title ?? card?.title : part === 'body' ? shown?.body ?? card?.body : null);
      return rect && rect.width && rect.height ? { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 } : null;
    }, { name, part });
    return Boolean(point);
  }, { message: `${name}'s ${part} drawn` }).toBe(true);
  return point;
}

/** Its name where a hand takes it, to drag it by or click to rename it: the old canvas's hover card's name, the new canvas's drawn title. */
async function titlePoint(page, name) {
  if (await newCanvas(page)) return partPoint(page, name, 'title');
  return middle((await hoverBox(page, name)).locator('.nm'));
}

/** Its name field, opened as a person opens it, a click on its name, and focused. The same NameField on both canvases. */
async function renameField(page, name) {
  const at = await titlePoint(page, name);
  await page.mouse.click(at.x, at.y);
  const field = page.getByRole('textbox', { name: 'Name', exact: true });
  await expect(field).toBeFocused();
  return field;
}

/** Its trash where a hand presses it: the old canvas's hover card's Delete, the new canvas's drawn one. */
async function trashPoint(page, name) {
  if (await newCanvas(page)) return partPoint(page, name, 'delete');
  const head = await middle((await hoverBox(page, name)).locator('.nm'));
  await page.mouse.move(head.x, head.y);
  return middle((await hoverBox(page, name)).getByRole('button', { name: 'Delete', exact: true }));
}

/** Its corner, shown with the pointer on it: the old canvas's hover card's grip, the new canvas's drawn one. */
async function cornerPoint(page, name) {
  if (!(await newCanvas(page))) return middle((await hoverBox(page, name)).locator('.corner').first());
  await hoverBox(page, name);
  let point = null;
  await expect.poll(async () => {
    point = await page.evaluate((name) => {
      const rect = document.querySelector('.canvas-root').canvasInspection.card(name)?.rendered?.parts?.corner;
      return rect && rect.width && rect.height ? { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 } : null;
    }, name);
    return Boolean(point);
  }, { message: `${name}'s corner drawn as it's hovered` }).toBe(true);
  return point;
}

/** The shape said as a corner is pulled: the old canvas's `.ratio-live`, the new canvas's ratio label on the card being pulled. Null when none. */
const liveRatio = (page) => page.evaluate(() => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  if (typeof inspection?.cards === 'function') {
    const name = inspection.interaction().resizing;
    return name ? inspection.card(name)?.rendered?.labels?.ratio?.text ?? null : null;
  }
  const said = document.querySelector('.ratio-live');
  return said ? said.innerText.trim() : null;
});

/** Its hover preview as a person sees it. Old canvas: the hover card's `<video>`, its time, muted and loop. New: the canvas's preview texture,
 *  up once its first frame is (no `<video>`: muted and loop are the scene's, null here). */
const previewState = (page, name) => page.evaluate((name) => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  if (typeof inspection?.cards === 'function') {
    const preview = inspection.card(name)?.rendered?.preview;
    return { canvas: 'new', mounted: Boolean(preview?.active), playing: Boolean(preview?.active && preview?.ready), at: null, muted: preview?.muted ?? null, loop: preview?.loop ?? null };
  }
  const video = document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}] video.preview.on`);
  return { canvas: 'old', mounted: Boolean(video), playing: Boolean(video && video.currentTime > 0.3), at: video?.currentTime ?? null, muted: video?.muted ?? null, loop: video?.loop ?? null };
}, name);

/** A long name with the pointer on it: whether it's cut at rest, what it says of itself under the pointer (the old canvas's hover card's
 *  title attribute; the new canvas's drawn title, or another label saying it whole), and whether the pointer is on the name. */
async function nameUnderPointer(page, name) {
  if (!(await newCanvas(page))) {
    const nm = (await hoverBox(page, name)).locator('.head .nm');
    const point = await middle(nm);
    await page.mouse.move(point.x, point.y);
    return nm.evaluate((nm, name) => {
      const r = nm.getBoundingClientRect(), hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { name, cut: nm.scrollWidth > nm.clientWidth, title: nm.title, whole: nm.scrollWidth <= nm.clientWidth + 1, text: nm.textContent.trim(), under_pointer: hit === nm || nm.contains(hit) };
    }, name);
  }
  const point = await partPoint(page, name, 'title');
  const rest = await page.evaluate((name) => document.querySelector('.canvas-root').canvasInspection.card(name)?.rendered?.labels?.title ?? null, name);
  // Hovered first, then the pointer on its title, as a person reads a name (hoverBox rests on the body).
  await hoverBox(page, name);
  await page.mouse.move(point.x, point.y);
  await expect.poll(() => page.evaluate((name) => {
    const state = document.querySelector('.canvas-root').canvasInspection.interaction?.()?.state?.();
    return !state || (state.hover?.name === name && state.hover?.part === 'title');
  }, name), { message: `${name}'s title under the pointer` }).toBe(true);
  // The canvas redraws a hovered title whole on its next frame: read it after two.
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  return page.evaluate(({ name, point, rest }) => {
    const inspection = document.querySelector('.canvas-root').canvasInspection, labels = inspection.card(name)?.rendered?.labels ?? {};
    const title = labels.title, full = title?.fullText ?? rest?.fullText ?? name;
    // The whole name, said where the pointer is: the canvas's own tooltip (its root's title) or a label drawn whole.
    const tooltip = document.querySelector('.canvas-root').getAttribute('title') ?? '';
    const said = tooltip === full || title?.text === full || Object.entries(labels).some(([key, label]) => key !== 'title' && label?.text === full);
    const hit = inspection.hitTest(point);
    return { name, cut: Boolean(rest) && rest.text !== rest.fullText, title: said ? full : tooltip || title?.text || '', whole: title?.text === full, text: title?.text ?? '', under_pointer: hit?.name === name && hit?.part === 'title' };
  }, { name, point, rest });
}

/** How many hover previews play anywhere on the canvas. */
const previewsPlaying = (page) => page.evaluate(() => {
  const inspection = document.querySelector('.canvas-root')?.canvasInspection;
  if (typeof inspection?.cards === 'function') return inspection.cards().filter((one) => inspection.card(one.name)?.rendered?.preview?.active).length;
  return document.querySelectorAll('video.preview.on').length;
});

/** The canvas's scale now. */
const zoomOf = (page) => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.camera().zoom ?? null);

/** No camera/Focus move or input barrier under way; three unchanged camera samples, so a not-yet-started transition cannot pass. */
async function canvasIdle(page) {
  let previous, still = 0;
  await expect.poll(async () => {
    const state = await page.evaluate(() => {
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      return { ready: Boolean(inspection?.resources()?.frames), camera: inspection?.camera(), interaction: inspection?.interaction(), moving: Boolean(document.querySelector('.stage[data-moving], [data-focused-asset][data-entering]')) };
    });
    const current = JSON.stringify(state.camera);
    const busy = !state.ready || state.moving || !state.camera || state.interaction?.locked || state.interaction?.pointerDown || state.interaction?.dragging || state.interaction?.marquee;
    still = !busy && current === previous ? still + 1 : 0;
    previous = current;
    return still >= 3;
  }, { message: 'the canvas is still and accepts input', intervals: [100] }).toBe(true);
}

/** Double-click the real card body; Focus names it and finishes drawing in. */
async function focus(page, name) {
  await canvasIdle(page);
  const region = page.getByRole('region', { name: 'Focused asset' });
  if (await region.count()) {
    if (await region.getAttribute('data-focused-asset') === name) return region;
    await region.getByRole('button', { name: 'Back to canvas', exact: true }).click();
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
  }
  const { x, y } = await canvasPoint(page, name);
  await page.mouse.dblclick(x, y);
  await expect(region).toHaveAttribute('data-focused-asset', name);
  await canvasIdle(page);
  return region;
}

/** A point on the actual hit canvas, clear of cards and native overlays. */
const openPoint = async (page) => {
  let point;
  await expect.poll(async () => {
    point = await page.evaluate(() => {
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      if (!inspection) return null;
      for (let y = innerHeight - 60; y > 120; y -= 30) for (let x = innerWidth - 60; x > 320; x -= 30) {
        if (document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !inspection.hitTest({ x, y })) return { x, y };
      }
      return null;
    });
    return Boolean(point);
  }, { message: 'open canvas available to point at' }).toBe(true);
  return point;
};

/** Every displayed card that moved, resized, came or went over ms. Explicit selectors observe native controls instead. */
const observeStill = (page, ms, selector = null) => page.evaluate(({ ms, selector }) => new Promise((done) => {
  const look = () => {
    if (selector) return new Map([...document.querySelectorAll(selector)].map((el) => {
      const { x, y, width, height } = el.getBoundingClientRect();
      return [el.dataset.asset ?? el.id, [x, y, width, height]];
    }));
    const inspection = document.querySelector('.canvas-root')?.canvasInspection;
    if (!inspection) throw new Error('canvas unavailable while observing its geometry');
    return new Map(inspection.names().flatMap(name => {
      const card = inspection.card(name), native = document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}]`);
      const r = native?.querySelector('.media')?.getBoundingClientRect() ?? card?.rendered?.body;
      return r ? [[name, [r.x, r.y, r.width, r.height]]] : [];
    }));
  };
  const began = performance.now(), changes = [];
  let last = look();
  const frame = (now) => {
    const seen = look(), at_ms = Math.round(now - began);
    for (const [name, rect] of seen) {
      const was = last.get(name);
      if (!was) changes.push({ at_ms, name, came: rect.map(Math.round) });
      else if (was.some((value, i) => Math.abs(value - rect[i]) > 0.5)) changes.push({ at_ms, name, from: was.map(Math.round), to: rect.map(Math.round) });
    }
    for (const name of last.keys()) if (!seen.has(name)) changes.push({ at_ms, name, went: true });
    last = seen;
    if (now - began < ms && changes.length < 50) requestAnimationFrame(frame);
    else done(changes);
  };
  requestAnimationFrame(frame);
}), { ms, selector });

/** An init script: in each document, every move of its address that changes its path (`window.__moves`) and each gate drawn
 *  (`window.__gates`, their words), for `stays`. */
function recordAddress() {
  window.__moves = [];
  for (const name of ['pushState', 'replaceState']) {
    const native = history[name].bind(history);
    history[name] = (state, title, url) => {
      const from = location.pathname;
      native(state, title, url);
      if (location.pathname !== from) window.__moves.push(`${from} -> ${location.pathname}`);
    };
  }
  window.__gates = [];
  new MutationObserver((records) => {
    for (const record of records) for (const node of record.addedNodes) if (node instanceof Element && node.matches('main.gate')) window.__gates.push(node.textContent);
  }).observe(document, { childList: true, subtree: true });
}

/** Where a page recording its address stands 3 s on, long enough for one that sends itself somewhere to have done it (cl-980's loop went
 *  round several times a second), and each move of its address since its document loaded: the other declared window. */
const stays = (page) => page.evaluate((ms) => new Promise((done) => setTimeout(() => done({ path: location.pathname, moves: window.__moves }), ms)), 3000);

/** The keys that put the caret at the start and the end of what's typed: macOS binds no Meta+Home or Meta+End (Home and End scroll there). */
const TEXT_START = process.platform === 'darwin' ? 'Meta+ArrowUp' : 'Control+Home';
const TEXT_END = process.platform === 'darwin' ? 'Meta+ArrowDown' : 'Control+End';

/** A local fixture tool (e2e/cli/project/<file>/<function>), as a box or `cozy run` names it. */
const fixtureTool = (path) => join(__dirname, '..', 'project', path);

module.exports = { test, expect, canvasCard, canvasNames, canvasText, canvasLabel, canvasPoint, canvasVisible, selectedNames, hoverBox, screenshotPixels, box,
  newCanvas, partPoint, titlePoint, renameField, trashPoint, cornerPoint, liveRatio, previewState, previewsPlaying, nameUnderPointer, canvasIdle, focus, middle, observeStill, openPoint, recordAddress, stays, zoomOf, fixtureTool, TEXT_END, TEXT_START, DARK_OPEN, MP4, photo, png, wav };
