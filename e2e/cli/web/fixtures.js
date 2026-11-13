// The web suite's fixtures (cl-973): each cell on a project and a browser context of its own, on this job's Calcifer, several at once.
// Guards fail a cell on a page error, a console error, a CSP violation or a modal. Waits are on state; `observeStill` is the one window.
//   const { test, expect, box, ... } = require('./fixtures')
// $HOLDER: Calcifer's lock record; $COZY: the CLI; $OUT: where the job keeps what it made; $ARGV_LOG: each cozy call, for coverage.
const { test: base, expect } = require('@playwright/test');
const { execFile } = require('child_process');
const { createHash, randomBytes } = require('crypto');
const { appendFileSync, readFileSync, rmSync } = require('fs');
const { loadavg } = require('os');
const { basename, join } = require('path');
const { DARK_OPEN, MP4, photo, png, wav } = require('./media');

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

/** A folder this suite made, once nothing holds it: Windows refuses one Calcifer still reads. */
async function removeFolder(folder) {
  if (!basename(folder).startsWith(MADE)) throw new Error(`refusing to remove ${folder}: the web suite didn't make it`);
  await expect.poll(() => {
    try {
      rmSync(folder, { recursive: true, force: true });
      return 'removed';
    } catch (error) {
      if (error.code !== 'EPERM' && error.code !== 'EBUSY') throw error;
      return error.code;
    }
  }, { message: `${folder} removed`, timeout: 60_000 }).toBe('removed');
}

/** In every page of a guarded context: CSP violations and modal dialogs, said to the fixture as they happen. */
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
  const boxOf = (el) => el?.closest?.('.react-flow__node')?.querySelector('.card')?.dataset.asset ?? null;
  let under = null;
  document.addEventListener('pointerover', ({ target, clientX, clientY }) => {
    const now = boxOf(target);
    if (now !== under) say(`pointer ${under} -> ${now} at ${clientX},${clientY}`);
    under = now;
  }, true);
  for (const type of ['mousedown', 'mouseup', 'click', 'dblclick', 'keydown', 'pointercancel', 'dragstart']) {
    document.addEventListener(type, ({ target, key }) => say(`${type}${key ? ` ${key}` : ''} ${target.tagName}.${target.className}`), true);
  }
  document.addEventListener('focusin', ({ target }) => say(`focus ${boxOf(target)} ${target.tagName}.${target.className}`), true);
  new MutationObserver((records) => {
    for (const { type, target, attributeName, oldValue, addedNodes, removedNodes } of records) {
      for (const node of addedNodes) if (node.classList?.contains('react-flow__node')) say(`added ${boxOf(node)} ${node.style.transform}`);
      for (const node of removedNodes) if (node.classList?.contains('react-flow__node')) say(`removed ${node.dataset.id}`);
      if (type !== 'attributes') continue;
      if (attributeName === 'data-moving') say(`canvas ${target.hasAttribute('data-moving') ? 'moving' : 'still'}`);
      if (attributeName === 'aria-selected') say(`${target.dataset.asset} selected=${target.getAttribute('aria-selected')}`);
      if (attributeName === 'style' && !oldValue?.includes(target.style.transform)) {
        if (target.classList.contains('react-flow__viewport')) say(`canvas at ${target.style.transform}`);
        if (target.classList.contains('react-flow__node')) say(`${boxOf(target)} at ${target.style.transform}`);
      }
    }
  }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['style', 'data-moving', 'aria-selected'], attributeOldValue: true });
}

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

  /** `test.use({ projectOptions: { starter: true } })`: the starter film; `encrypted: true`: a vault, its passphrase on `project`. */
  projectOptions: [{ starter: false, encrypted: false }, { option: true }],

  project: async ({ api, projectOptions, leftovers }, use, testInfo) => {
    const folder = join(need('OUT'), 'web', 'projects', `${MADE}${slug(testInfo.title)}-${randomBytes(4).toString('hex')}`);
    leftovers.push(folder);
    const passphrase = projectOptions.encrypted ? randomBytes(18).toString('base64url') : undefined;
    const body = { path: folder, starter: projectOptions.starter, ...(passphrase ? { passphrase } : { unencrypted: true }) };
    // Calcifer refuses a creation while another is being made, as if a source capture held it (409 `source_capture_busy`: creation takes
    // its fence by try_write, so two at once collide): asked again, backing off, as its message says to. Anything else is the cell's failure.
    let opened;
    for (let wait = 100, until = Date.now() + 60_000; ; wait = Math.min(wait * 2, 2000)) {
      const answer = await api.raw('POST', '/v1/projects', { body });
      const text = await answer.text();
      if (answer.ok) {
        opened = JSON.parse(text);
        break;
      }
      if (answer.status !== 409 || !text.includes('"source_capture_busy"') || Date.now() > until) throw new Error(`POST /v1/projects: ${answer.status} ${text.slice(0, 500)}`);
      await new Promise((done) => setTimeout(done, wait));
    }
    await use(new Project(api, opened, passphrase));
  },

  /** Fails the cell on what any of its pages said: a page error, a console error, a CSP violation, a modal. `allow(re)` takes what a cell
   *  brings about on purpose, matched against `<message> <url>`. */
  guards: async ({}, use) => {
    const said = [];
    const allowed = [];
    const say = (line) => { if (!allowed.some((re) => re.test(line))) said.push(line); };
    await use({
      allow: (...patterns) => { allowed.push(...patterns); },
      async watch(context) {
        await context.exposeBinding('__cozyGuard', (_, line) => say(line));
        await context.addInitScript(guardPage);
        const watch = (page) => {
          page.on('pageerror', (error) => say(`page error: ${error.message}`));
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
    await context.addInitScript(watchBoxes);
    await use(context);
  },

  /** Another browser context, as another person's browser: guarded, closed after the cell. */
  newContext: async ({ browser, guards }, use) => {
    const made = [];
    await use(async (options = {}) => {
      const context = await browser.newContext(options);
      made.push(context);
      await guards.watch(context);
      return context;
    });
    for (const context of made) await context.close();
  },

  /** Signs a tab in as `cozy open` does: a code from the holder, redeemed at `to`, which the page clears from its address. Its session. */
  signIn: async ({ api }, use) => {
    await use(async (page, to) => {
      const { code } = await api.post('/v1/session-codes', {});
      // A page reads its code as it loads: at the address it's on, only the fragment would change, and nothing loads.
      if (page.url() !== 'about:blank') await page.goto('about:blank');
      const redeemed = page.waitForResponse((answer) => answer.request().method() === 'POST' && new URL(answer.url()).pathname === '/v1/session');
      await page.goto(`${to.replace(/#.*$/, '')}#code=${encodeURIComponent(code)}`);
      const answer = await redeemed;
      expect(answer.status(), 'POST /v1/session').toBe(200);
      await expect(page, 'the code leaves the address').not.toHaveURL(/[#&]code=/);
      return answer.json();
    });
  },

  /** A tab signed in at the cell's project, its canvas up. The box log is attached when the cell fails. */
  page: async ({ page, project, signIn }, use, testInfo) => {
    await signIn(page, project.url());
    await expect(page.locator('.stage')).toBeVisible();
    await use(page);
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

/** A box on the canvas by its name: its card, which says `aria-selected`. */
const box = (page, name) => page.locator(`.react-flow__node .card[data-asset=${JSON.stringify(name)}]`);

/** The middle of what `locator` shows on screen, where a person points: a locator's own click can scroll the canvas from under it. */
async function middle(locator) {
  const { x, y, width, height } = await locator.boundingBox();
  return { x: x + width / 2, y: y + height / 2 };
}

/** The canvas's scale now. */
const zoomOf = (page) => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);

/** The canvas still: no camera move under way (`data-moving`) and no Focus drawing in. */
async function canvasIdle(page) {
  await expect(page.locator('.stage')).not.toHaveAttribute('data-moving');
  await expect(page.locator('[data-focused-asset][data-entering]')).toHaveCount(0);
}

/** A box opened by a double click on its middle: Focus names it, has drawn in, and the canvas is still. Focus's region. */
async function focus(page, name) {
  const { x, y } = await middle(box(page, name).locator('.media').first());
  await page.mouse.dblclick(x, y);
  const region = page.getByRole('region', { name: 'Focused asset' });
  await expect(region).toHaveAttribute('data-focused-asset', name);
  await canvasIdle(page);
  return region;
}

/** A point on open canvas, clear of every box and what floats over it. */
const openPoint = (page) => page.evaluate(() => {
  for (let y = innerHeight - 60; y > 120; y -= 30) for (let x = innerWidth - 60; x > 320; x -= 30) if (document.elementFromPoint(x, y)?.classList.contains('react-flow__pane')) return { x, y };
  throw new Error('no open canvas to point at');
});

/** Every box that moved, resized, came or went over `ms`, frame by frame in the page: [] when nothing did. The one declared window. */
const observeStill = (page, ms, selector = '.react-flow__node .card[data-asset]') => page.evaluate(({ ms, selector }) => new Promise((done) => {
  const look = () => new Map([...document.querySelectorAll(selector)].map((el) => {
    const { x, y, width, height } = el.getBoundingClientRect();
    return [el.dataset.asset ?? el.id, [x, y, width, height]];
  }));
  const began = performance.now();
  const changes = [];
  let last = look();
  const frame = (now) => {
    const seen = look();
    const at_ms = Math.round(now - began);
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

/** A local fixture tool (e2e/cli/project/<file>/<function>), as a box or `cozy run` names it. */
const fixtureTool = (path) => join(__dirname, '..', 'project', path);

module.exports = { test, expect, box, canvasIdle, focus, middle, observeStill, openPoint, zoomOf, fixtureTool, DARK_OPEN, MP4, photo, png, wav };
