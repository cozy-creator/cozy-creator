// walkflow (release-gate/sessions/walk-flow.sh; the 0.27.0 launch walkthrough's user flow): a new user's path on ONE rental, through
// Calcifer's own page at a laptop's size, as user-flow.spec.js drives its two (its recorders, waits and hands are that cell's):
//   a project made from the page (encrypted, its recovery secret saved); an image box and H3's Shot dragged from the Tools palette, their
//   words typed, the Shot's beat cut to $SHOT_S s; $SKU with $GPUS GPUs rented from the Machines panel, preferred, followed to ready; the
//   Shot rendered; the run's tile, the Runs panel and the machine's drawer (its GPUs, its queue) looked at while it runs; the picture
//   rendered while the Shot goes; both again, warm; each take seen in Focus, the video played, each downloaded by a right click; the
//   rental ended from its drawer.
// It records: the page's event stream as it reads it (events.ndjson), what the page said (seen.ndjson), each run's events by `cozy --json
// run watch` and its full record, the machine's record, a screenshot at each step (shots/), and flow.json: each request's times from its
// click (answered, accepted, started, first step, settled, its take in the project). Without $FLOW_RENT: the same up to the quote.
const { spawn } = require('child_process');
const { appendFileSync, closeSync, mkdirSync, openSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { randomBytes } = require('crypto');
const { test, expect, canvasVisible, canvasPoint, focus, canvasIdle } = require('./fixtures');
const { fit, leave } = require('./hands');
const { carry } = require('./geometry');

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: release-gate/sessions/walk-flow.sh sets it`); })();
const FLOW = need('FLOW_OUT');
const RENT = process.env.FLOW_RENT === 'true';
const SKU = need('SKU');
const GPUS = Number(process.env.GPUS || 2);
const SHOT_S = Number(process.env.SHOT_S || 5);
const IMAGE = process.env.IMAGE_TOOL || 'flux.3-klein/image';
const QUIET_MS = Number(process.env.QUIET_MIN || 20) * 60_000;
const STEP_MS = 90 * 60_000;
const GENERATE = 'minimax-h3/generate';
const SUMMARY = 'A lighthouse on a rocky coast at dusk.';
const BEAT = 'The beam sweeps over the dark sea as waves roll in and break on the rocks.';
const PICTURE = 'a red fox curled up asleep in fresh snow under a pine tree, soft morning light, photograph';

/** Open canvas with a box's room around it, left to right from under the bar (user-flow.spec.js's). */
async function roomAt(page) {
  let point;
  await expect.poll(async () => {
    point = await page.evaluate(() => {
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      if (!inspection) return null;
      const open = (x, y) => document.elementFromPoint(x, y)?.classList.contains('canvas-pane') && !inspection.hitTest({ x, y });
      for (let y = 160; y < innerHeight - 340; y += 40) for (let x = 520; x < innerWidth - 240; x += 40) {
        let clear = true;
        for (let dy = -40; clear && dy <= 320; dy += 40) for (let dx = -220; clear && dx <= 220; dx += 40) clear = open(x + dx, y + dy);
        if (clear) return { x, y };
      }
      return null;
    });
    return Boolean(point);
  }, { message: "open canvas with a box's room", timeout: 30_000 }).toBe(true);
  return point;
}

/** In every document, before the page's own scripts (user-flow.spec.js's): its `GET /v1/events` read twice, ours handed to `__flowEvent`
 *  cut to what is read here; and what the page shows, handed to `__flowSeen` when it changes. */
function recorders() {
  const cut = (event) => {
    if (event.type === 'run.updated') {
      const r = event.run;
      return { type: event.type, run: { id: r.id, name: r.name, number: r.number, kind: r.kind, target: r.target, machine: r.machine, project: r.project, project_id: r.project_id, status: r.status, machine_state: r.machine_state, waiting: r.waiting, queue_position: r.queue_position, progress: r.progress, arriving: r.arriving, reason: r.reason, created_at_ms: r.created_at_ms, accepted_at_ms: r.accepted_at_ms, started_at_ms: r.started_at_ms, settled_at_ms: r.settled_at_ms, placed: r.placed, gpu: r.gpu } };
    }
    if (event.type === 'machine.updated') {
      const m = event.machine, s = m.status ?? {};
      return { type: event.type, machine: { name: m.name, kind: m.kind, state: m.state, rental_id: m.rental_id, running: m.running, queued: m.queued, reason: m.reason, broken: m.broken, unreachable: m.unreachable, rental: m.rental && { sku: m.rental.sku, gpus: m.rental.gpus, disk_gb: m.rental.disk_gb, usd_micros_per_hour: m.rental.usd_micros_per_hour }, activity: s.activity, models_bytes: s.models_bytes, runtime: s.runtime, tensorfs: s.tensorfs, gpus: s.gpus, queue: s.queue?.map((q) => ({ id: q.id, run: q.run, target: q.target, state: q.state, gpus: q.gpus })) } };
    }
    return { type: event.type, id: event.id ?? event.name ?? event.stream };
  };
  const real = window.fetch;
  window.fetch = async function (input, init) {
    const response = await real.call(this, input, init);
    try {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (url.pathname !== '/v1/events' || (init?.method ?? 'GET') !== 'GET' || !response.body) return response;
      const [ours, theirs] = response.body.tee();
      (async () => {
        const reader = ours.pipeThrough(new TextDecoderStream()).getReader();
        let buffer = '', data = [];
        for (;;) {
          const { value, done } = await reader.read();
          if (done) return;
          buffer += value;
          for (let end = buffer.indexOf('\n'); end >= 0; end = buffer.indexOf('\n')) {
            const line = buffer.slice(0, end).replace(/\r$/, '');
            buffer = buffer.slice(end + 1);
            if (line === '' && data.length) {
              try { window.__flowEvent?.(Date.now(), JSON.stringify(cut(JSON.parse(data.join('\n'))))); } catch { /* a comment or a partial frame */ }
              data = [];
            } else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
          }
        }
      })().catch(() => {});
      return new Response(theirs, { status: response.status, statusText: response.statusText, headers: response.headers });
    } catch {
      return response;
    }
  };
  const said = new Map();
  const send = (key, text) => { if (said.get(key) !== text) { said.set(key, text); window.__flowSeen?.(Date.now(), key, text); } };
  const words = (el) => (el?.innerText ?? '').replace(/\s+/g, ' ').trim();
  setInterval(() => {
    try {
      const region = document.querySelector('[data-focused-asset]');
      const focused = region?.getAttribute('data-focused-asset') ?? '';
      send('focus', focused);
      if (region) {
        const tiles = [...region.querySelectorAll('.here-side .take-at')].map((at) => {
          const tile = at.querySelector(':scope > .take');
          if (!tile) return null;
          return { run: tile.dataset.run ?? null, making: tile.classList.contains('making'), title: tile.getAttribute('title'), phase: tile.querySelector('.take-phase')?.textContent ?? null, share: tile.querySelector('.take-said')?.textContent ?? null, where: tile.querySelector('.take-where')?.textContent ?? null, text: words(tile) };
        }).filter(Boolean);
        send(`tiles ${focused}`, JSON.stringify(tiles));
        send(`beside render ${focused}`, [...region.querySelectorAll('.ready-on, .render-time, .slow-here')].map(words).filter(Boolean).join(' · '));
      }
      for (const face of document.querySelectorAll('[role=status][aria-label$="\'s render"]')) send(`face ${face.getAttribute('aria-label')}`, words(face));
      for (const note of document.querySelectorAll('.notes .note')) send(`note ${note.dataset.note ?? words(note).slice(0, 40)}`, words(note));
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      for (const name of inspection?.names?.() ?? []) {
        const labels = inspection.card(name)?.rendered?.labels;
        if (labels?.progress || labels?.count) send(`canvas ${name}`, [labels.progress?.fullText, labels.count?.fullText, labels.time?.fullText].filter(Boolean).join(' | '));
      }
    } catch (error) {
      send('recorder error', String(error));
    }
  }, 500);
}

test.use({ viewport: { width: 1440, height: 900 } });

test('walkflow', { tag: '@flow' }, async ({ context, api, signIn, guards }) => {
  test.setTimeout(3 * 3600_000);
  guards.allow(/./);
  for (const dir of ['runs', 'machines', 'shots', 'downloads', 'text']) mkdirSync(join(FLOW, dir), { recursive: true });
  const flow = { cozy: process.env.COZY_VERSION ?? '?', rent: RENT, sku: SKU, gpus: GPUS, image: IMAGE, shot_s: SHOT_S, steps: [], machine: null, sent: [], times: [], said: [], failures: [], errors: [] };
  const keep = () => writeFileSync(join(FLOW, 'flow.json'), JSON.stringify(flow, null, 2));
  const mark = (what, extra = {}) => { flow.steps.push({ at: Date.now(), what, ...extra }); keep(); };
  const failed = (what, error) => { flow.failures.push(`${what}: ${String(error?.message ?? error).split('\n').slice(0, 3).join(' | ').slice(0, 500)}`); keep(); };
  const saw = (line) => { flow.said.push(line.replace(/\s+/g, ' ').slice(0, 800)); keep(); };
  let shotN = 0;
  let page;
  const shot = async (name) => { if (page && !page.isClosed()) await page.screenshot({ path: join(FLOW, 'shots', `${String(++shotN).padStart(2, '0')}-${name}.png`) }).catch(() => {}); };
  const text = async (name, locator) => writeFileSync(join(FLOW, 'text', `${name}.txt`), await locator.innerText().catch((error) => `(no text: ${error.message})`));
  /** A step that says its failure and lets the flow go on. */
  const soft = (what, fn) => test.step(what, fn).catch(async (error) => { failed(what, error); await shot(`FAILED-${what.toLowerCase().replace(/[^a-z0-9]+/g, '-').slice(0, 40)}`); });

  // The project the page makes: its id, code and folder once made.
  let project = null;
  const at = (rest = '') => `/v1/projects/${encodeURIComponent(project.id)}${rest}`;
  const assets = async () => (await api.get(at())).assets;
  const asset = async (name) => (await assets()).find((row) => row.name === name);
  const url = (rest = '') => `${api.origin}/p/${project.code}${rest ? `/${rest}` : ''}`;

  // The page's stream, as it read it.
  const runs = new Map(), machines = new Map(), last = new Map(), waiters = new Set(), watching = new Set(), firsts = new Map();
  let heard = 0, movedAt = Date.now();
  const mine = (run) => project && (run.project_id === project.id || run.project === project.name);
  const ours = (one) => Boolean(flow.machine) && ((flow.machine.name && flow.machine.name === one.name) || (flow.machine.rental_id && flow.machine.rental_id === one.rental_id));
  const watch = (run) => {
    const name = run.name || String(run.number);
    if (watching.has(name)) return;
    watching.add(name);
    const out = openSync(join(FLOW, 'runs', `${name}.ndjson`), 'a');
    const child = spawn(need('COZY'), ['--json', 'run', 'watch', name], { cwd: project.path, stdio: ['ignore', out, out] });
    child.on('exit', () => closeSync(out));
  };
  await context.exposeBinding('__flowEvent', (source, when, line) => {
    heard += 1;
    appendFileSync(join(FLOW, 'events.ndjson'), `{"at":${when},"event":${line}}\n`);
    const event = JSON.parse(line);
    if (event.type === 'machine.removed') {
      const was = machines.get(event.id);
      machines.delete(event.id);
      if (flow.machine && !flow.machine.left_at && (flow.machine.name === event.id || (flow.machine.rental_id && was?.rental_id === flow.machine.rental_id))) { flow.machine.left_at = when; flow.machine.left_state = was?.state ?? null; keep(); }
      for (const waiter of waiters) waiter.moved(true);
      return;
    }
    const key = event.run ? `run ${event.run.id}` : event.machine ? `machine ${event.machine.name}` : null;
    if (!key) return;
    const said = JSON.stringify(event.run ?? event.machine);
    if (last.get(key) === said) return;
    last.set(key, said);
    if (event.run) {
      const run = event.run;
      runs.set(run.id, { ...run, seen_at: runs.get(run.id)?.seen_at ?? when });
      if (mine(run) && run.kind === 'call') {
        watch(run);
        // When the page first heard each thing of it: its machine, running, its first progress, its first step, its end.
        const first = firsts.get(run.id) ?? {};
        const p = run.progress;
        if (run.machine && !first.machine) first.machine = { at: when, machine: run.machine };
        if (run.status === 'in_progress' && !first.in_progress) first.in_progress = when;
        if (p && !first.progress) first.progress = { at: when, stage: p.stage };
        if (p && !(p.unit ?? '') && p.total > 0 && p.completed >= 1 && !first.step) first.step = { at: when, stage: p.stage, completed: p.completed, total: p.total, step_ms: p.step_ms };
        if (p?.step_steady_ms) first.steady_ms = p.step_steady_ms;
        if (['completed', 'failed', 'canceled'].includes(run.status) && !first.settled) first.settled = when;
        firsts.set(run.id, first);
      }
    } else machines.set(event.machine.name, event.machine);
    if (event.run ? mine(event.run) : ours(event.machine)) movedAt = Date.now();
    for (const waiter of waiters) waiter.moved(event.run ? mine(event.run) : ours(event.machine));
  });
  await context.exposeBinding('__flowSeen', (source, when, key, said) => appendFileSync(join(FLOW, 'seen.ndjson'), `${JSON.stringify({ at: when, key, text: said })}\n`));
  await context.addInitScript(recorders);

  const until = (what, holds, quiet = QUIET_MS) => new Promise((done, fail) => {
    const began = Date.now();
    let moved = began;
    const waiter = {
      moved(ourOwn = true) {
        if (ourOwn) moved = Date.now();
        try { const got = holds(); if (got) { stop(); done(got); } } catch (error) { stop(); fail(error); }
      },
    };
    const timer = setInterval(() => {
      if (Date.now() - moved > quiet) { stop(); fail(new Error(`${what}: nothing moved for ${quiet / 60_000} min`)); }
      else if (Date.now() - began > STEP_MS) { stop(); fail(new Error(`${what}: not in ${STEP_MS / 60_000} min`)); }
      else waiter.moved(false);
    }, 5_000);
    const stop = () => { clearInterval(timer); waiters.delete(waiter); };
    waiters.add(waiter);
    waiter.moved();
  });
  const callRuns = () => [...runs.values()].filter((run) => mine(run) && run.kind === 'call');
  const settled = (run) => ['completed', 'failed', 'canceled'].includes(run.status);
  const machineNow = () => flow.machine && [...machines.values()].find((one) => (flow.machine.rental_id && one.rental_id === flow.machine.rental_id) || one.name === flow.machine.name);
  const toolRow = async (groups, id) => {
    const tools = page.getByRole('complementary', { name: 'Tools' });
    const row = tools.locator(`li[data-implementation][data-tool=${JSON.stringify(id)}]`);
    await expect.poll(async () => {
      for (const group of groups) {
        if (await row.isVisible()) return true;
        const button = tools.getByRole('button', { name: group, exact: true });
        if (!(await button.count())) continue;
        if ((await button.getAttribute('aria-expanded')) !== 'true') await button.click();
        if (await row.isVisible()) return true;
      }
      return false;
    }, { message: `${id} in Tools › ${groups.join(' or ')}`, timeout: 60_000, intervals: [500, 1000] }).toBe(true);
    return row;
  };
  const look = async (name) => {
    const open = page.getByRole('region', { name: 'Focused asset' });
    if ((await open.count()) && (await open.getAttribute('data-focused-asset')) === name) return open;
    if (await open.count()) await leave(page);
    await fit(page);
    return focus(page, name);
  };
  const home = async () => { await page.goto(url()); await expect(page.locator('.stage')).toBeVisible({ timeout: 60_000 }); await canvasIdle(page).catch(() => {}); };
  const prefer = async () => {
    const box = page.locator('.machine-head input.render-check');
    await expect(box, 'the machine can be preferred').toBeEnabled({ timeout: 60_000 });
    if (!(await box.isChecked())) await box.check();
    await expect(box).toBeChecked();
    flow.machine.preferred_at = flow.machine.preferred_at ?? Date.now();
  };
  /** A box rendered from Focus by its Render: its new run, by the page's stream; its times are measured from the click. */
  const render = async (request, box, target) => {
    const before = new Set(callRuns().map((run) => run.id));
    const r = await look(box);
    const beside = (await r.locator('.ready-on, .render-time, .slow-here').allTextContents().catch(() => [])).map((t) => t.trim()).filter(Boolean).join(' · ');
    const versions = (await asset(box))?.versions ?? 0;
    const asked = page.waitForResponse((a) => a.request().method() === 'POST' && /\/v1\/projects\/[^/]+\/runs$/.test(new URL(a.url()).pathname), { timeout: 120_000 });
    const sent = { request, box, target, beside_render: beside, versions_before: versions, at: Date.now() };
    flow.sent.push(sent);
    await r.getByRole('button', { name: 'Render', exact: true }).click();
    const answer = await asked;
    sent.answered_ms = Date.now() - sent.at;
    sent.answer = { status: answer.status(), body: await answer.json().catch(() => null) };
    keep();
    const run = await until(`request ${request}'s run`, () => callRuns().find((one) => !before.has(one.id) && one.target?.includes(target)));
    Object.assign(sent, { run: run.id, name: run.name, number: run.number });
    mark(`request ${request} is ${run.name}`, { box, target });
    return sent;
  };
  /** A request's times, from its click, once its run has settled and its take is in the project (or isn't). */
  const timesOf = async (sent) => {
    const run = runs.get(sent.run), first = firsts.get(sent.run) ?? {};
    let take = null;
    if (run?.status === 'completed') {
      await expect.poll(async () => ((await asset(sent.box))?.versions ?? 0), { timeout: 600_000, intervals: [1000] }).toBeGreaterThan(sent.versions_before).then(() => { take = Date.now(); }, () => {});
    }
    const s = (when) => (when ? Math.round((when - sent.at) / 100) / 10 : null);
    const line = {
      request: sent.request, box: sent.box, run: sent.name, target: sent.target, status: run?.status, machine: run?.machine ?? null, reason: run?.reason ?? null, beside_render: sent.beside_render,
      answered_s: sent.answered_ms / 1000, accepted_s: s(run?.accepted_at_ms), on_machine_s: s(first.machine?.at), started_s: s(run?.started_at_ms), first_progress_s: s(first.progress?.at), first_progress_stage: first.progress?.stage ?? null,
      first_step_s: s(first.step?.at), first_step: first.step ? `${first.step.stage} ${first.step.completed}/${first.step.total}, ${Math.round(first.step.step_ms)} ms` : null, steady_step_ms: first.steady_ms ?? null,
      settled_s: s(run?.settled_at_ms), heard_settled_s: s(first.settled), take_in_project_s: s(take),
    };
    flow.times.push(line);
    keep();
    return line;
  };
  /** The machine's drawer, as it stands: a screenshot, its words, its GPUs as the page's stream last said them. */
  const drawer = async (name) => {
    await page.goto(url(`machines/${encodeURIComponent(flow.machine.name)}`));
    await page.waitForTimeout(3500);
    await shot(`drawer-${name}`);
    await text(`drawer-${name}`, page.locator('body'));
    const now = machineNow();
    saw(`drawer ${name}: running ${now?.running}, queued ${now?.queued}; GPUs ${JSON.stringify(now?.gpus ?? []).slice(0, 900)}; queue ${JSON.stringify(now?.queue ?? null).slice(0, 500)}`);
    writeFileSync(join(FLOW, 'machines', `status-${name}.json`), JSON.stringify(now ?? null, null, 2));
  };
  const runsPanel = async (name) => {
    await page.goto(url('runs'));
    await page.waitForTimeout(2500);
    await shot(`runs-${name}`);
    await text(`runs-${name}`, page.locator('body'));
  };

  page = await context.newPage();
  page.on('pageerror', (error) => { flow.errors.push({ at: Date.now(), line: `page error: ${error.message}`.slice(0, 400) }); keep(); });
  page.on('console', (message) => { if (message.type() === 'error') { flow.errors.push({ at: Date.now(), line: `console error: ${message.text()} ${message.location().url}`.slice(0, 400) }); keep(); } });
  page.on('response', (r) => { if (r.status() >= 400) { flow.errors.push({ at: Date.now(), line: `HTTP ${r.status()} ${r.request().method()} ${new URL(r.url()).pathname}`.slice(0, 300) }); keep(); } });
  let SHOT = null, PIC = null;
  try {
    await test.step('a project made from the page: named, encrypted, its recovery secret saved', async () => {
      const t0 = Date.now();
      await signIn(page, `${api.origin}/projects`);
      await page.waitForTimeout(1500);
      await shot('projects');
      const before = new Set(((await api.get('/v1/projects')).projects ?? []).map((p) => p.path));
      await page.getByRole('button', { name: 'New project', exact: true }).first().click();
      const form = page.getByRole('region', { name: 'New project' });
      const name = `first-film-${randomBytes(3).toString('hex')}`;
      await page.getByLabel("New project's name", { exact: true }).fill(name);
      const passphrase = randomBytes(18).toString('base64url');
      await page.getByLabel('Passphrase', { exact: true }).fill(passphrase);
      await page.getByLabel('Confirm passphrase', { exact: true }).fill(passphrase);
      await shot('new-project-form');
      await text('new-project-form', (await form.count()) ? form : page.locator('body'));
      await page.getByRole('button', { name: 'Create project', exact: true }).click();
      const secret = page.getByRole('region', { name: 'Save recovery secret' });
      await expect(secret).toBeVisible({ timeout: 120_000 });
      await shot('recovery-secret');
      await secret.getByRole('checkbox').check();
      await secret.getByRole('button', { name: 'Done', exact: true }).click();
      await expect(page.locator('.stage')).toBeVisible({ timeout: 120_000 });
      await page.waitForURL(/\/p\/[^/]+$/, { timeout: 60_000 }).catch(() => {});
      const listed = (await api.get('/v1/projects')).projects ?? [];
      const made = listed.find((p) => !before.has(p.path) && p.path.endsWith(`/${name}`)) ?? listed.find((p) => !before.has(p.path));
      if (!made) throw new Error(`no new project listed; the page is at ${page.url()}`);
      project = { id: made.id, code: made.code ?? new URL(page.url()).pathname.split('/')[2], path: made.path, name: made.name ?? name };
      flow.project = { ...project, made_s: (Date.now() - t0) / 1000, at_url: new URL(page.url()).pathname };
      await canvasIdle(page).catch(() => {});
      await shot('empty-canvas');
      mark('project made', { path: project.path });
    });

    let generate, image;
    await test.step('the image tool and the Shot from the Tools palette, their words typed', async () => {
      const listing = Date.now();
      await expect.poll(async () => {
        const { tools } = await api.get(`/v1/tools?on=local&project=${encodeURIComponent(project.id)}`);
        [generate, image] = [GENERATE, IMAGE].map((id) => tools.find((tool) => tool.id === id || tool.id.endsWith(`/${id}`)));
        return Boolean(generate && image);
      }, { message: `the palette lists ${GENERATE} and ${IMAGE}`, timeout: 1_200_000, intervals: [2_000, 5_000] }).toBe(true);
      flow.tools = [generate, image].map((tool) => ({ id: tool.id, version: tool.version ?? tool.release ?? null, source: tool.source ?? null, label: tool.label ?? null }));
      flow.tools_listed_s = (Date.now() - listing) / 1000;
      const drop = async (groups, tool) => {
        const before = new Set((await assets()).map((row) => row.name));
        await fit(page).catch(() => {});
        const row = await toolRow(groups, tool.id).catch(async () => { await home(); return toolRow(groups, tool.id); });
        await carry(page, row, await roomAt(page));
        let name = null;
        await expect.poll(async () => { name = (await assets()).find((row) => !before.has(row.name) && row.tool === tool.id)?.name ?? null; return name; }, { message: `${tool.id}'s box made`, timeout: 60_000 }).not.toBeNull();
        await expect.poll(() => canvasVisible(page, name), { timeout: 60_000 }).toBe(true);
        return name;
      };
      PIC = await drop(['Image'], image);
      await shot('picture-box-made');
      let r = await look(PIC);
      await r.locator('.mention-text').first().click();
      await page.keyboard.type(PICTURE);
      await expect.poll(async () => JSON.stringify((await asset(PIC))?.fields ?? {}), { message: "the picture's words saved", timeout: 30_000 }).toContain('red fox');
      await page.waitForTimeout(800);
      await shot('picture-typed');
      await text('picture-focus', r);
      await leave(page);
      SHOT = await drop(['Video'], generate);
      await shot('shot-box-made');
      r = await look(SHOT);
      await r.getByRole('textbox', { name: 'Summary', exact: true }).click();
      await page.keyboard.type(SUMMARY);
      const toggle = r.locator('.beat-toggle[aria-controls$="-0"]').first();
      if ((await toggle.count()) && (await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
      await r.locator('.mention-text[data-field="beats.0.description"]').click();
      await page.keyboard.type(BEAT);
      await expect.poll(async () => (await asset(SHOT))?.fields?.beats?.[0]?.description, { message: 'its beat saved', timeout: 30_000 }).toBe(BEAT);
      // A short one: the beat's length pulled down, where the page offers it.
      const duration = r.getByRole('slider', { name: "Beat 1's duration", exact: true });
      if (await duration.isVisible().catch(() => false)) {
        await duration.focus();
        await page.keyboard.press('Home');
        for (let s = 1; s < SHOT_S; s++) await page.keyboard.press('ArrowRight');
        await expect.poll(async () => (await asset(SHOT))?.fields?.beats?.[0]?.duration, { message: `its beat ${SHOT_S} s`, timeout: 30_000 }).toBe(SHOT_S).catch((error) => failed(`the Shot's beat set to ${SHOT_S} s`, error));
      } else failed(`the Shot's beat set to ${SHOT_S} s`, "no \"Beat 1's duration\" slider in Focus");
      await page.waitForTimeout(800);
      await shot('shot-typed');
      await text('shot-focus', r);
      flow.boxes = { [PIC]: (await asset(PIC)).fields, [SHOT]: (await asset(SHOT)).fields };
      keep();
      await leave(page);
      await fit(page).catch(() => {});
      await shot('canvas-two-boxes');
      mark('boxes made', { picture: PIC, shot: SHOT });
    });

    await expect.poll(() => heard, { message: "the page's event stream, as the page reads it", timeout: 60_000 }).toBeGreaterThan(0);
    await test.step(`${SKU} x${GPUS} from the Machines panel: its row, its price, Rent`, async () => {
      flow.machine = { sku: SKU, gpus: GPUS, asked_at: Date.now() };
      await page.goto(url('machines'));
      const row = page.locator('li.sku').filter({ has: page.locator(`b:text-is(${JSON.stringify(SKU)})`) });
      await expect(row, `${SKU} is offered`).toBeVisible({ timeout: 120_000 });
      const width = row.getByRole('combobox', { name: new RegExp(`^(GPUs for ${SKU}|${SKU}: GPUs)$`) });
      if (await width.count()) await width.selectOption(String(GPUS));
      else if (GPUS !== 1) throw new Error(`${SKU} is offered with one GPU only`);
      await page.waitForTimeout(800);
      flow.machine.row = (await row.innerText()).replace(/\s+/g, ' ');
      await shot('machines-panel');
      await text('machines-panel', page.locator('body'));
      await row.getByRole('button', { name: 'Rent', exact: true }).click();
      const primary = page.locator('.order .primary');
      await expect(primary, 'a price stated').toBeEnabled({ timeout: 120_000 });
      flow.machine.stated = (await primary.textContent()).trim();
      flow.machine.order_says = (await page.locator('.order').innerText()).replace(/\s+/g, ' ');
      await shot('quote');
      saw(`the Machines row: ${flow.machine.row}; the order: ${flow.machine.order_says}`);
      if (!RENT) {
        await page.locator('.order').getByRole('button', { name: 'Not now', exact: true }).click();
        mark('pre-flight: nothing ordered');
        return;
      }
      const answered = page.waitForResponse((r) => r.request().method() === 'POST' && new URL(r.url()).pathname === '/v1/machines', { timeout: 600_000 });
      flow.machine.ordered_at = Date.now();
      await primary.click();
      const answer = await answered;
      const said = await answer.json().catch(() => ({}));
      if (!answer.ok()) throw new Error(`the order: ${answer.status()} ${JSON.stringify(said).slice(0, 500)}`);
      Object.assign(flow.machine, { name: said.machine?.name, rental_id: said.machine?.rental_id ?? null, answered_at: Date.now() });
      writeFileSync(join(FLOW, 'rental'), `${flow.machine.rental_id ?? flow.machine.name}\n`);
      mark('ordered', { name: flow.machine.name, rental_id: flow.machine.rental_id });
      await page.waitForURL(/\/machines\/.+/, { timeout: 60_000 }).catch(() => {});
      await prefer().catch((error) => failed('preferred as it boots', error));
      await shot('ordered-booting');
      await text('drawer-booting', page.locator('body'));
    });
    if (!RENT) return;

    await test.step('the machine boots: followed on the page until it is ready, still preferred', async () => {
      const now = await until('the machine ready', () => {
        const it = machineNow();
        if (it?.rental_id) flow.machine.rental_id = it.rental_id;
        if (it?.state === 'ended') throw new Error('the machine ended while it booted');
        return it?.state === 'ready' ? it : null;
      });
      Object.assign(flow.machine, { ordered_as: flow.machine.name, name: now.name, ready_at: Date.now(), boot_s: (Date.now() - flow.machine.ordered_at) / 1000, disk_gb: now.rental?.disk_gb, usd_micros_per_hour: now.rental?.usd_micros_per_hour, gpu_count: (now.gpus ?? []).length, runtime: now.runtime, tensorfs: now.tensorfs });
      mark('ready', { name: flow.machine.name });
      await page.goto(url(`machines/${encodeURIComponent(flow.machine.name)}`));
      await prefer();
      await page.waitForTimeout(2000);
      await shot('ready-preferred');
      await text('drawer-ready', page.locator('body'));
      writeFileSync(join(FLOW, 'text', 'project-machines.json'), JSON.stringify((await api.get(at())).machines ?? null));
      saw(`ready: ${flow.machine.name}, ${flow.machine.gpu_count} GPUs, ${flow.machine.disk_gb} GB, ${flow.machine.usd_micros_per_hour} micro-USD/h, booted in ${Math.round(flow.machine.boot_s)} s; the project's machines ${JSON.stringify((await api.get(at())).machines ?? null)}`);
      await home();
    });

    const first = {};
    await test.step('request 1: the Shot rendered; its tile, the Runs panel and the drawer while it runs', async () => {
      first.shot = await render(1, SHOT, GENERATE);
      await page.waitForTimeout(4000);
      await shot('request-1-sent');
      await runsPanel('shot-queued');
      await drawer('shot-early');
      await home();
    });
    await test.step('request 2: the picture rendered while the Shot goes', async () => {
      const going = runs.get(first.shot.run);
      saw(`as the picture is asked, the Shot's run is ${going?.status} on ${going?.machine ?? 'no machine yet'}${going?.progress ? `, ${going.progress.stage} ${going.progress.completed}/${going.progress.total}` : ''}`);
      first.pic = await render(2, PIC, IMAGE);
      await page.waitForTimeout(4000);
      await shot('request-2-sent');
      await runsPanel('both-asked');
      await drawer('both-asked');
    });
    await test.step('both run: the drawer and the Runs panel once each is going, until both settle', async () => {
      // A look each time one of them moves on (a machine, running, a first step), at most one a minute, until both settle.
      let looks = 0, lastLook = 0, lastState = '';
      for (;;) {
        const pair = [first.shot, first.pic].map((sent) => runs.get(sent.run));
        if (pair.every((run) => run && settled(run))) break;
        const state = pair.map((run) => `${run?.status}:${run?.progress?.stage ?? ''}:${run?.machine ?? ''}`).join('|');
        if (looks < 14 && (state !== lastState || Date.now() - lastLook > 120_000) && Date.now() - lastLook > 45_000) {
          lastState = state; lastLook = Date.now(); looks += 1;
          saw(`look ${looks}: ${pair.map((run) => `${run?.name} ${run?.status}${run?.progress ? ` ${run.progress.stage} ${run.progress.completed}/${run.progress.total}` : ''}${run?.waiting ? ` waiting: ${JSON.stringify(run.waiting).slice(0, 160)}` : ''}`).join('; ')}`);
          await drawer(`look-${looks}`);
          if (looks % 3 === 1) await runsPanel(`look-${looks}`);
          await home();
          await look(SHOT).catch(() => {});
          await page.waitForTimeout(1500);
          await shot(`shot-focus-look-${looks}`);
        }
        await page.waitForTimeout(3000);
        if (Date.now() - movedAt > QUIET_MS) throw new Error(`nothing of this project's runs or its machine moved for ${QUIET_MS / 60_000} min: ${state}`);
        if (Date.now() - flow.sent[0].at > STEP_MS) throw new Error(`not settled in ${STEP_MS / 60_000} min: ${state}`);
      }
      for (const sent of [first.shot, first.pic]) saw(`cold ${JSON.stringify(await timesOf(sent))}`);
      await drawer('after-first-pair');
    });

    await soft('both again, warm: the Shot, then the picture while it goes', async () => {
      await home();
      const again = { shot: await render(3, SHOT, GENERATE) };
      await page.waitForTimeout(2500);
      again.pic = await render(4, PIC, IMAGE);
      await page.waitForTimeout(6000);
      await drawer('warm-both');
      await runsPanel('warm-both');
      await until('the warm pair settles', () => ([again.shot, again.pic].every((sent) => settled(runs.get(sent.run) ?? {})) ? true : null));
      for (const sent of [again.shot, again.pic]) saw(`warm ${JSON.stringify(await timesOf(sent))}`);
    });

    await soft('the results: the canvas, the video in Focus played, the picture in Focus', async () => {
      await home();
      await fit(page).catch(() => {});
      await page.waitForTimeout(2500);
      await shot('canvas-results');
      const r = await look(SHOT);
      await expect(r.locator('.focus-image-transform > .media')).toBeVisible({ timeout: 60_000 });
      await page.waitForTimeout(1500);
      await shot('shot-in-focus');
      await text('shot-in-focus', r);
      await r.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
      const live = r.locator('video.live');
      await expect.poll(async () => (await live.count()) && live.evaluate((el) => el.currentTime), { message: 'it plays', timeout: 60_000 }).toBeGreaterThan(0.2);
      const video = await live.evaluate((el) => ({ natural: [el.videoWidth, el.videoHeight], duration: el.duration, at: el.currentTime, paused: el.paused, error: el.error?.code ?? null }));
      await shot('shot-playing');
      saw(`the Shot's take plays in Focus: ${JSON.stringify(video)}`);
      await r.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click().catch(() => {});
      const p = await look(PIC);
      await page.waitForTimeout(1500);
      await shot('picture-in-focus');
      await text('picture-in-focus', p);
    });

    await soft('each result onto the disk: a right click on its take in Focus, and on its box on the canvas', async () => {
      for (const name of [PIC, SHOT]) {
        const r = await look(name);
        const tile = r.locator('[data-take]').first();
        await tile.click({ button: 'right' });
        await page.waitForTimeout(600);
        await shot(`take-menu-${name === PIC ? 'picture' : 'shot'}`);
        const item = page.getByRole('menuitem', { name: /download/i }).or(page.getByRole('button', { name: /^download$/i })).first();
        const [file] = await Promise.all([page.waitForEvent('download', { timeout: 60_000 }), item.click()]);
        await file.saveAs(join(FLOW, 'downloads', `take-${file.suggestedFilename()}`));
        saw(`downloaded by a right click on ${name}'s take: ${file.suggestedFilename()}`);
      }
      await leave(page).catch(() => {});
      await fit(page).catch(() => {});
      for (const name of [PIC, SHOT]) {
        const where = await canvasPoint(page, name);
        await page.mouse.click(where.x, where.y, { button: 'right' });
        await page.waitForTimeout(600);
        if (name === PIC) { await shot('box-menu'); writeFileSync(join(FLOW, 'text', 'box-menu.txt'), await page.locator('[role=menu]').allInnerTexts().then((t) => t.join('\n')).catch(() => '(no menu)')); }
        const item = page.getByRole('menuitem', { name: /download/i }).first();
        const [file] = await Promise.all([page.waitForEvent('download', { timeout: 60_000 }), item.click()]);
        await file.saveAs(join(FLOW, 'downloads', `box-${file.suggestedFilename()}`));
        saw(`downloaded by a right click on ${name}'s box: ${file.suggestedFilename()}`);
      }
    });
  } catch (error) {
    failed('the flow', error);
    await shot('failed');
    throw error;
  } finally {
    if (RENT && project) {
      await soft("each run's record and the machine's, before it ends", async () => {
        const save = (file, ...words) => new Promise((done) => {
          const out = openSync(join(FLOW, file), 'w');
          const child = spawn(need('COZY'), words, { cwd: project.path, stdio: ['ignore', out, out] });
          child.on('exit', (code) => { closeSync(out); done(code); });
        });
        for (const run of callRuns()) await save(`runs/${run.name || run.number}.json`, 'run', 'watch', run.name || String(run.number), '--once', '--json', '--full');
        await save('runs.json', '--json', 'run', 'ls', '--limit', '0');
        await save('runs.txt', 'run', 'ls', '--limit', '0');
        if (flow.machine?.name) {
          await save('machines/show.json', '--json', 'machine', 'show', flow.machine.name);
          await save('machines/show.txt', 'machine', 'show', flow.machine.name);
          await save('machines/models.txt', 'model', 'ls', '--on', flow.machine.name);
          await save('machines/tensord.log', 'machine', 'logs', flow.machine.name, 'tensord', '--since', '2h', '-n', '1500');
        }
        mark('collected');
      });
      await soft('the rental ended from its drawer', async () => {
        const m = flow.machine;
        if (!m?.name && !m?.rental_id) return;
        const now = machineNow();
        if (!now || ['ending', 'ended'].includes(now.state)) { saw(`before End was pressed the machine was ${now?.state ?? 'gone from the list'}`); return; }
        await page.goto(url(`machines/${encodeURIComponent(m.name ?? m.rental_id)}`));
        await page.waitForTimeout(1500);
        const actions = page.locator('.machine-actions');
        await actions.getByRole('button', { name: 'End', exact: true }).click({ timeout: 60_000 });
        const confirm = page.locator('.confirm');
        await expect(confirm).toBeVisible();
        m.end_says = (await confirm.locator('.says').textContent().catch(() => '')).trim();
        await shot('end-confirm');
        await confirm.getByRole('button', { name: /^(End rental|End it)$/ }).click();
        m.end_pressed_at = Date.now();
        await page.waitForTimeout(2500);
        await shot('ending');
        await until('the machine ended', () => (!machineNow() || machineNow().state === 'ended' ? true : null));
        m.ended_at = Date.now();
        await page.waitForTimeout(1500);
        await shot('ended');
        await page.goto(url('machines'));
        await page.waitForTimeout(2500);
        await shot('machines-after-end');
        await text('machines-after-end', page.locator('body'));
        saw(`End: "${m.end_says}"; gone from the page's list ${Math.round((m.ended_at - m.end_pressed_at) / 100) / 10} s after End rental`);
      });
    }
    keep();
  }
});
