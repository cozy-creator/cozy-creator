// user-flow (xs-203), the release gate's main suite. Paul, 11-16: "open webui, decrypt project, spawn node, attach tool, rent machine, send
// request to do inference, send another request, rent another machine, send another request, and make sure that the whole thing works and
// is roughly as fast and efficient as it can be." Run by release-gate/sessions/user-flow.sh (pod-session), on tensorhub.com with real
// rentals, through Calcifer's own page as a person uses it:
//   an encrypted project, locked, opened from the Projects list and unlocked with its passphrase; a plate (a video) dropped on the canvas;
//   H3's Shot dragged from Tools › Video onto open canvas and its words typed; H3's Edit from Tools › Video dropped on the plate, VFX
//   edit chosen, its beat as long as the plate; machine A rented through the Machines panel ($SKU, one GPU) and preferred; the Shot
//   rendered; the plate's edit rendered, behind it on A; machine B rented ($SKU_B, $GPUS_B GPUs) and preferred as it boots (Paul had both
//   preferred); the Shot rendered again while B boots, and once more when B is ready. Calcifer places each (no machine is named): cl-1073's
//   proof on real pods (a busy machine's time left counted, a waiting run moved to one clearly sooner, a run on every GPU of B and the next
//   one queued or elsewhere, a machine updating not free). Every take lands in the project and shows in Focus; both machines end through
//   the page.
// It records, and release-gate/sessions/fixtures/user-flow/judge.py judges: the page's own event stream as it reads it (events.ndjson:
// run and machine changes, downloads and steps), what the page said (seen.ndjson: Focus's take tiles, run faces, toasts, sampled
// every 500 ms), each run's events by `cozy --json run watch` and its full record, each machine's record and TensorD log, screenshots
// at each step. Without $FLOW_RENT it spends nothing: the same up to the Machines panel's quotes, and no order.
// $FLOW_OUT: where it keeps what it saw. $SKU, $SKU_B, $GPUS_B, $PLATE (an mp4), $PLATE_S, $QUIET_MIN.
const { spawn } = require('child_process');
const { appendFileSync, closeSync, copyFileSync, mkdirSync, openSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasVisible, canvasPoint, focus, canvasIdle } = require('./fixtures');
const { dropFiles, fit, leave } = require('./hands');
const { carry } = require('./geometry');

const need = (name) => process.env[name] || (() => { throw new Error(`$${name} is not set: release-gate/sessions/user-flow.sh sets it`); })();
const FLOW = need('FLOW_OUT');
const RENT = process.env.FLOW_RENT === 'true';
const SKU = need('SKU');
const SKU_B = process.env.SKU_B || SKU;
const GPUS_B = Number(process.env.GPUS_B || 2);
const PLATE_S = Number(process.env.PLATE_S || 5);
// A wait fails when nothing it watches has moved this long (Paul 11-15: never for slowness, only on evidence of a hang), or past the
// session's budget for one step.
const QUIET_MS = Number(process.env.QUIET_MIN || 20) * 60_000;
const STEP_MS = 2 * 3600_000;
// How long request 1 waits for its box to say "Ready on A" (cl-1104): well under a rental's 15 idle minutes.
const READY_MS = 8 * 60_000;
const GENERATE = 'minimax-h3/generate';
const EDIT = 'minimax-h3/edit';
// The 0.25.0 gate's clip (bench.json): its words, at Shot's default 15 s.
const SUMMARY = 'A lighthouse on a rocky coast at dusk.';
const BEAT = 'The beam sweeps over the dark sea as waves roll in and break on the rocks. The scene is set at dawn.';
// The h3 suite's: the VFX edit LoRA asked to change nothing of a synthetic test pattern.
const GUIDE = 'The video is kept exactly as it is, with nothing changed. A synthetic broadcast test pattern fills the frame: colour bars, a moving gradient sweep, a turning marker and a running timecode on a dark field. The camera is static and no person appears.';

/** Open canvas with a box's room around it (a video box is 360 wide), clear of every card and control, left to right from under the bar:
 *  where a person lets go of a new box. fixtures.js's openPoint scans from the bottom right corner, where a box lands half off screen. */
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

/** In every document, before the page's own scripts: its `GET /v1/events` read twice, the page's copy untouched and ours handed to
 *  `__flowEvent` one `data:` block at a time, cut to what the judge reads (a machine's snapshot is large); and what the page shows,
 *  looked at every 500 ms and handed to `__flowSeen` when it changes. */
function recorders() {
  const cut = (event) => {
    if (event.type === 'run.updated') {
      const r = event.run;
      return { type: event.type, run: { id: r.id, name: r.name, number: r.number, kind: r.kind, target: r.target, machine: r.machine, project: r.project, project_id: r.project_id, status: r.status, machine_state: r.machine_state, waiting: r.waiting, queue_position: r.queue_position, progress: r.progress, arriving: r.arriving, reason: r.reason, created_at_ms: r.created_at_ms, accepted_at_ms: r.accepted_at_ms, started_at_ms: r.started_at_ms, settled_at_ms: r.settled_at_ms, placed: r.placed, cancel_requested: r.cancel_requested, stalled: r.stalled } };
    }
    if (event.type === 'machine.updated') {
      const m = event.machine, s = m.status ?? {};
      return { type: event.type, machine: { name: m.name, kind: m.kind, state: m.state, rental_id: m.rental_id, running: m.running, queued: m.queued, offline: m.offline, broken: m.broken, unreachable: m.unreachable, rental: m.rental && { sku: m.rental.sku, gpus: m.rental.gpus, disk_gb: m.rental.disk_gb, usd_micros_per_hour: m.rental.usd_micros_per_hour }, activity: s.activity, disk: s.disk, models_bytes: s.models_bytes, runtime: s.runtime, tensorfs: s.tensorfs, gpus: (s.gpus ?? []).map((gpu) => gpu.name), queue: s.queue?.map((q) => ({ id: q.id, run: q.run, target: q.target, state: q.state })) } };
    }
    return { type: event.type, id: event.id ?? event.name ?? event.stream };
  };
  const real = window.fetch;
  window.fetch = async function (input, init) {
    const response = await real.call(this, input, init);
    try {
      // The page passes a URL object (transport.ts `resolve`), a string or a Request.
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
          const off = at.querySelector(':scope > .take-off');
          return { run: tile.dataset.run ?? null, making: tile.classList.contains('making'), title: tile.getAttribute('title'), phase: tile.querySelector('.take-phase')?.textContent ?? null, share: tile.querySelector('.take-said')?.textContent ?? null, where: tile.querySelector('.take-where')?.textContent ?? null, text: words(tile), cancel: off?.getAttribute('aria-label') ?? null };
        }).filter(Boolean);
        send(`tiles ${focused}`, JSON.stringify(tiles));
      }
      for (const face of document.querySelectorAll('[role=status][aria-label$="\'s render"]')) send(`face ${face.getAttribute('aria-label')}`, words(face));
      for (const note of document.querySelectorAll('.notes .note')) send(`note ${note.dataset.note ?? words(note).slice(0, 40)}`, words(note));
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      for (const name of inspection?.names?.() ?? []) {
        const labels = inspection.card(name)?.rendered?.labels;
        if (labels?.progress || labels?.count) send(`canvas ${name}`, [labels.progress?.fullText, labels.count?.fullText, labels.time?.fullText].filter(Boolean).join(' | '));
      }
      const body = document.body?.innerText ?? '';
      const rate = body.match(/[^\n]{0,60}\bMBps\b[^\n]{0,20}/);
      if (rate) send('MBps', rate[0]);
      const none = body.match(/[^\n]{0,60}\b0 (of|\/) 0\b[^\n]{0,30}/);
      if (none) send('0 of 0', none[0]);
    } catch (error) {
      send('recorder error', String(error));
    }
  }, 500);
}

test.use({ projectOptions: { encrypted: true }, viewport: { width: 1600, height: 1000 } });

test('user-flow', { tag: '@flow' }, async ({ context, project, api, cli, signIn, guards }) => {
  test.setTimeout(4 * 3600_000);
  for (const dir of ['runs', 'machines', 'shots']) mkdirSync(join(FLOW, dir), { recursive: true });
  const holder = () => JSON.parse(readFileSync(need('HOLDER'), 'utf8'));
  const flow = { cozy: process.env.COZY_VERSION ?? '?', rent: RENT, sku: SKU, sku_b: SKU_B, gpus_b: GPUS_B, plate_s: PLATE_S, project: { id: project.id, path: project.path }, calcifer: { pid_at_start: holder().pid }, steps: [], machines: {}, sent: [], loads: [], failures: [], errors: [] };
  const keep = () => writeFileSync(join(FLOW, 'flow.json'), JSON.stringify(flow, null, 2));
  const mark = (what, extra = {}) => { flow.steps.push({ at: Date.now(), what, ...extra }); keep(); };
  /** A step's failure is said and the flow goes on where it can: collection and End always come. */
  const failed = (what, error) => { flow.failures.push(`${what}: ${String(error?.message ?? error).split('\n')[0].slice(0, 400)}`); keep(); };
  let shotN = 0;
  let page;
  const shot = async (name) => { if (page && !page.isClosed()) await page.screenshot({ path: join(FLOW, 'shots', `${String(++shotN).padStart(2, '0')}-${name}.png`) }).catch(() => {}); };

  // The page's stream, as it read it: each change of a run or machine, kept and judged. A wait is on these, never on a clock.
  const runs = new Map(), machines = new Map(), last = new Map(), waiters = new Set(), watching = new Set();
  let heard = 0;
  const mine = (run) => run.project_id === project.id || run.project === project.name;
  const ourMachine = (one) => Object.values(flow.machines).some((m) => (m.name && m.name === one.name) || (m.rental_id && m.rental_id === one.rental_id));
  const watch = (run) => {
    // Each run's own events, by the CLI, from its first sight: its progress frames aren't in its log after.
    const name = run.name || String(run.number);
    if (watching.has(name)) return;
    watching.add(name);
    const out = openSync(join(FLOW, 'runs', `${name}.ndjson`), 'a');
    const child = spawn(need('COZY'), ['--json', 'run', 'watch', name], { cwd: project.path, stdio: ['ignore', out, out] });
    child.on('exit', () => closeSync(out));
  };
  await context.exposeBinding('__flowEvent', (source, at, line) => {
    heard += 1;
    appendFileSync(join(FLOW, 'events.ndjson'), `{"at":${at},"event":${line}}\n`);
    const event = JSON.parse(line);
    if (event.type === 'machine.removed') {
      const was = machines.get(event.id);
      machines.delete(event.id);
      // One of ours gone from the list before End was pressed ended by itself (a rental idle 15 min releases itself): when, and its last state.
      for (const m of Object.values(flow.machines)) {
        if (m.left_at || m.end_pressed_at || machineOf(m) || !(m.name === event.id || (m.rental_id && was?.rental_id === m.rental_id))) continue;
        Object.assign(m, { left_at: at, left_state: was?.state ?? null });
        keep();
      }
      for (const waiter of waiters) waiter.moved(ourMachine({ name: event.id, rental_id: was?.rental_id }));
      return;
    }
    const key = event.run ? `run ${event.run.id}` : event.machine ? `machine ${event.machine.name}` : null;
    if (!key) return;
    const said = JSON.stringify(event.run ?? event.machine);
    if (last.get(key) === said) return;
    last.set(key, said);
    if (event.run) {
      runs.set(event.run.id, { ...event.run, seen_at: runs.get(event.run.id)?.seen_at ?? at });
      if (mine(event.run) && event.run.kind === 'call') watch(event.run);
    } else machines.set(event.machine.name, event.machine);
    // The account's other machines and other projects' runs are on this stream too: only ours say this flow is still moving.
    for (const waiter of waiters) waiter.moved(event.run ? mine(event.run) : ourMachine(event.machine));
  });
  await context.exposeBinding('__flowSeen', (source, at, key, text) => appendFileSync(join(FLOW, 'seen.ndjson'), `${JSON.stringify({ at, key, text })}\n`));
  await context.addInitScript(recorders);

  /** Until `holds()` answers, each time what the page reads changes; fails once nothing of ours (this project's runs, the machines it
   *  rented) has for QUIET_MS, or after STEP_MS. */
  const until = (what, holds) => new Promise((done, fail) => {
    const began = Date.now();
    let moved = began;
    const waiter = {
      moved(ours = true) {
        if (ours) moved = Date.now();
        try { const got = holds(); if (got) { stop(); done(got); } } catch (error) { stop(); fail(error); }
      },
    };
    const timer = setInterval(() => {
      if (Date.now() - moved > QUIET_MS) { stop(); fail(new Error(`${what}: nothing moved for ${QUIET_MS / 60_000} min`)); }
      if (Date.now() - began > STEP_MS) { stop(); fail(new Error(`${what}: not in ${STEP_MS / 60_000} min`)); }
    }, 5_000);
    const stop = () => { clearInterval(timer); waiters.delete(waiter); };
    waiters.add(waiter);
    waiter.moved();
  });
  const callRuns = () => [...runs.values()].filter((run) => mine(run) && run.kind === 'call');
  const settled = (run) => ['completed', 'failed', 'canceled'].includes(run.status);
  const machineOf = (m) => [...machines.values()].find((one) => (m.rental_id && one.rental_id === m.rental_id) || one.name === m.name);
  const goto = async (url) => { flow.loads.push({ at: Date.now(), url, asked: true }); await page.goto(url); };
  /** Beat `n`'s words in Focus `r`, its section opened first (a closed one is inert), typed as a person types them. */
  const typeBeat = async (r, n, words) => {
    const toggle = r.locator(`.beat-toggle[aria-controls$="-${n}"]`).first();
    if ((await toggle.count()) && (await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
    const beat = r.locator(`.mention-text[data-field="beats.${n}.description"]`);
    await beat.click();
    await page.keyboard.type(words);
  };
  /** Tool `id`'s row in the Tools palette, looked for in `groups` in turn: H3's Edit is in Video since #3515 and in Edit Video before
   *  (tonight's RC). A group shut again by a change to the page is opened again until the row shows. */
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
  /** Option `option` of choice `label` in Focus `r`, as the page draws it: the browser's own menu (since cl-1066), the page's dropdown of
   *  described options before it (tonight's RC, fc1956d16), or segments for a few. */
  const choose = async (r, label, option) => {
    const menu = r.getByRole('combobox', { name: label, exact: true });
    const dropdown = r.locator(`button[aria-haspopup="listbox"][aria-label=${JSON.stringify(label)}]`);
    await expect(menu.or(dropdown).or(r.getByRole('radiogroup', { name: label, exact: true })).first(), `the ${label} choice`).toBeVisible();
    if (await menu.count()) return menu.selectOption({ label: option });
    if (await dropdown.count()) {
      await dropdown.click();
      return r.getByRole('listbox', { name: label, exact: true }).getByRole('option').filter({ hasText: option }).first().click();
    }
    return r.getByRole('radiogroup', { name: label, exact: true }).getByRole('radio', { name: option, exact: true }).click();
  };
  /** Focus `r` on a new take's words: a box showing a take (the plate's own file) offers its fields under New (takes.spec.js). */
  const draft = async (r) => {
    const take = r.getByLabel('Take', { exact: true });
    if ((await take.count()) && (await take.textContent()) !== 'New take') {
      await r.locator('.focus-actions .focus-new').click();
      await expect(take).toHaveText('New take');
    }
  };
  /** Box `name` in Focus, as a person finds it: out of any other's Focus, the canvas fitted, a double-click on it. */
  const look = async (name) => {
    const open = page.getByRole('region', { name: 'Focused asset' });
    if ((await open.count()) && (await open.getAttribute('data-focused-asset')) === name) return open;
    if (await open.count()) await leave(page);
    await fit(page);
    return focus(page, name);
  };

  /** Ordered through the Machines panel as a person does it: the SKU's row, its GPUs, Rent, the stated price, one click; then preferred
   *  from its drawer as it boots (its "Prefer" checkbox: this project renders there). Without FLOW_RENT, the quote only. */
  const order = async (label, sku, gpus) => {
    const m = (flow.machines[label] = { label, sku, gpus, asked_at: Date.now() });
    await goto(`${project.url()}/machines`);
    const row = page.locator('li.sku').filter({ has: page.locator(`b:text-is(${JSON.stringify(sku)})`) });
    await expect(row, `${sku} is offered`).toBeVisible({ timeout: 120_000 });
    // "GPUs for <sku>" since cl-1084's words; "<sku>: GPUs" before (tonight's RC).
    const width = row.getByRole('combobox', { name: new RegExp(`^(GPUs for ${sku}|${sku}: GPUs)$`) });
    if (await width.count()) await width.selectOption(String(gpus));
    else if (gpus !== 1) throw new Error(`${sku} is offered with one GPU only`);
    await row.getByRole('button', { name: 'Rent', exact: true }).click();
    const primary = page.locator('.order .primary');
    await expect(primary, 'a price stated').toBeEnabled({ timeout: 120_000 });
    m.stated = (await primary.textContent()).trim();
    await shot(`${label}-quoted`);
    if (!RENT) {
      await page.locator('.order').getByRole('button', { name: 'Not now', exact: true }).click();
      return m;
    }
    const answered = page.waitForResponse((r) => r.request().method() === 'POST' && new URL(r.url()).pathname === '/v1/machines', { timeout: 600_000 });
    m.ordered_at = Date.now();
    await primary.click();
    const answer = await answered;
    const said = await answer.json().catch(() => ({}));
    if (!answer.ok()) throw new Error(`the order: ${answer.status()} ${JSON.stringify(said).slice(0, 400)}`);
    Object.assign(m, { name: said.machine?.name, rental_id: said.machine?.rental_id ?? null, answered_at: Date.now() });
    writeFileSync(join(FLOW, `rental-${label}`), `${m.rental_id ?? m.name}\n`);
    mark(`${label} ordered`, { name: m.name, rental_id: m.rental_id });
    // The page opens its drawer as it orders: preferred there while it boots.
    await page.waitForURL(/\/machines\/.+/, { timeout: 60_000 }).catch(() => {});
    await prefer(m).catch((error) => failed(`${label} preferred as it boots`, error));
    // Its Hub names its host a moment after the order (rent-… becomes its name, cl-1089). Still preferred under that name before the
    // next request is placed: its loss is said and it's checked again, so the request's placement is judged with both preferred.
    if (/^rent-[0-9a-f]+$/.test(m.name ?? '')) {
      const named = await expect.poll(() => { const it = machineOf(m); return it && it.name !== m.name ? it.name : null; }, { timeout: 60_000 }).not.toBeNull().then(() => machineOf(m)?.name ?? null, () => null);
      if (named) {
        Object.assign(m, { ordered_as: m.name, name: named });
        await goto(`${project.url()}/machines/${encodeURIComponent(named)}`);
        await prefer(m).catch((error) => failed(`${label} preferred under its Hub's name`, error));
      }
    }
    await shot(`${label}-ordered`);
    return m;
  };

  /** Its "Prefer" checkbox in its drawer, checked: this project renders there. */
  const prefer = async (m) => {
    const box = page.locator('.machine-head input.render-check');
    await expect(box, `${m.label} can be preferred`).toBeEnabled({ timeout: 60_000 });
    if (await box.isChecked()) return;
    // Checked as it booted, under the handle its order gave it: still checked once its Hub names its host (cl-1089's rename).
    if (m.preferred_at) failed(`${m.label}'s Prefer, checked as it booted, held through its rename`, `unchecked in ${m.name}'s drawer${m.ready_at ? ' once ready' : ', as its Hub named it'}`);
    await box.check();
    await expect(box).toBeChecked();
    m.preferred_at = m.preferred_at ?? Date.now();
    mark(`${m.label} preferred`, { name: m.name });
  };

  /** Followed by the page's stream until it's ready, then the project's canvas again. */
  const ready = async (m) => {
    const now = await until(`${m.label} ready`, () => {
      const it = machineOf(m);
      if (it?.rental_id) m.rental_id = it.rental_id;
      if (it?.state === 'ended') throw new Error(`${m.label} ended while it booted`);
      return it?.state === 'ready' ? it : null;
    });
    Object.assign(m, { name: now.name, ready_at: Date.now(), disk_gb: now.rental?.disk_gb, usd_micros_per_hour: now.rental?.usd_micros_per_hour, gpu_names: now.gpus });
    mark(`${m.label} ready`, { name: m.name });
    await goto(`${project.url()}/machines/${encodeURIComponent(m.name)}`);
    await prefer(m);
    await shot(`${m.label}-ready`);
    await goto(project.url());
    await expect(page.locator('.stage')).toBeVisible({ timeout: 60_000 });
    return m;
  };

  /** A box rendered from Focus by its Render, as the page sends it: its new run, by the page's stream. */
  const render = async (request, box, target) => {
    const before = new Set(callRuns().map((run) => run.id));
    const r = await look(box);
    const asked = page.waitForResponse((a) => a.request().method() === 'POST' && /\/v1\/projects\/[^/]+\/runs$/.test(new URL(a.url()).pathname), { timeout: 120_000 });
    const sent = { request, box, target, at: Date.now() };
    flow.sent.push(sent);
    await r.getByRole('button', { name: 'Render', exact: true }).click();
    const answer = await asked;
    sent.answer = { status: answer.status(), body: await answer.json().catch(() => null) };
    keep();
    const run = await until(`request ${request}'s run`, () => callRuns().find((one) => !before.has(one.id) && one.target?.includes(target)));
    Object.assign(sent, { run: run.id, name: run.name, number: run.number });
    mark(`request ${request} is ${run.name}`, { box, target });
    return run;
  };

  page = await context.newPage();
  page.on('domcontentloaded', () => flow.loads.push({ at: Date.now(), url: page.url() }));
  guards.allow(/status of 409\b/);
  // What the cell's guards fail on once the flow is over, kept for the judge's table too.
  const erred = (line) => { if (!/status of 409\b/.test(line)) { flow.errors.push({ at: Date.now(), line: line.slice(0, 400) }); keep(); } };
  page.on('pageerror', (error) => erred(`page error: ${error.message}`));
  page.on('console', (message) => { if (message.type() === 'error') erred(`console error: ${message.text()} ${message.location().url}`); });
  try {
    await test.step('the project locked, opened from the Projects list and unlocked with its passphrase', async () => {
      const locked = await cli('--json', 'vault', '--directory', project.path, 'lock');
      expect(locked.code, `cozy vault lock: ${locked.stderr.slice(-300)}`).toBe(0);
      await expect.poll(async () => (await api.get(project.at('/vault'))).state, { timeout: 60_000 }).toBe('locked');
      flow.loads.push({ at: Date.now(), url: '/projects', asked: true });
      await signIn(page, `${api.origin}/projects`);
      await page.locator('ul.list').locator(`a[title=${JSON.stringify(project.path)}]`).click();
      await expect(page.getByText('This project is locked.', { exact: true })).toBeVisible({ timeout: 60_000 });
      await shot('locked');
      mark('locked page');
      await page.getByLabel('Passphrase', { exact: true }).fill(project.passphrase);
      await page.getByRole('button', { name: 'Unlock project', exact: true }).click();
      await expect(page.locator('.stage')).toBeVisible({ timeout: 120_000 });
      await expect.poll(async () => (await api.get(project.at('/vault'))).state, { timeout: 60_000 }).toBe('unlocked');
      flow.code = new URL(page.url()).pathname;
      mark('unlocked');
      await shot('unlocked');
      // Unlocked where it was opened, it's the project: its canvas, not a page about its locked handle (43852897461: "This project can't
      // be opened yet", its uploads sent to `locked-…`). Said, then opened at its own address so the flow goes on.
      // The page's words for it: "Couldn't open this project" since cl-1084, "This project can't be opened yet" before. Either way the
      // tab stays at `locked-…`, which is no project's address.
      const stuck = page.getByRole('heading', { name: /^(Couldn't open this project|This project can't be opened yet)$/ });
      const stale = await stuck.waitFor({ state: 'visible', timeout: 10_000 }).then(() => true, () => false);
      if (stale || /\/locked-/.test(new URL(page.url()).pathname)) {
        failed('the project opens where it was unlocked, from the Projects list', `at ${new URL(page.url()).pathname}: ${(await page.locator('main, .stage').first().innerText()).replace(/\s+/g, ' ').slice(0, 300)}`);
        await goto(project.url());
        await expect(page.locator('.stage')).toBeVisible({ timeout: 60_000 });
        await expect(stuck).toHaveCount(0, { timeout: 30_000 });
        await shot('opened-at-its-address');
      }
    });

    await test.step('a plate dropped on the canvas: its box', async () => {
      await canvasIdle(page);
      await dropFiles(page, [{ name: 'plate.mp4', type: 'video/mp4', bytes: readFileSync(need('PLATE')) }], await roomAt(page));
      const naming = page.getByRole('textbox', { name: 'Name', exact: true });
      await expect(naming).toBeFocused({ timeout: 30_000 });
      await naming.fill('plate');
      await naming.press('Enter');
      await expect.poll(async () => (await project.asset('plate'))?.sha256 ?? null, { message: 'the plate held', timeout: 300_000 }).not.toBeNull();
      await expect.poll(() => canvasVisible(page, 'plate'), { timeout: 60_000 }).toBe(true);
      mark('plate dropped');
    });

    let SHOT, generate, edit;
    await test.step("H3's Shot from Tools › Video onto open canvas, its words typed", async () => {
      // The tools as the palette lists them, by the page's own question: the Hub's releases, or in a dev home the packages checkout's,
      // which Calcifer reads from source behind the list and lists once read (pre-flight 43956716291: none yet, 8 s after its start).
      const listing = Date.now();
      await expect.poll(async () => {
        const { tools } = await api.get(`/v1/tools?on=local&project=${encodeURIComponent(project.id)}`);
        [generate, edit] = [GENERATE, EDIT].map((id) => tools.find((tool) => tool.id === id || tool.id.endsWith(`/${id}`)));
        return Boolean(generate && edit);
      }, { message: `the palette lists ${GENERATE} and ${EDIT}`, timeout: 1_200_000, intervals: [2_000, 5_000] }).toBe(true);
      flow.tools = [generate, edit].map((tool) => tool && { id: tool.id, version: tool.version ?? tool.release ?? null, source: tool.source ?? null });
      flow.tools_listed_s = (Date.now() - listing) / 1000;
      keep();
      const before = new Set((await project.assets()).map((asset) => asset.name));
      await fit(page);
      // A tool listed after the page opened shows in its palette without a reload; one that doesn't is said, and the page loaded again.
      const row = await toolRow(['Video'], generate.id).catch(async (error) => {
        failed('the palette shows a tool listed after the page opened', error);
        await goto(project.url());
        await expect(page.locator('.stage')).toBeVisible({ timeout: 60_000 });
        await fit(page);
        return toolRow(['Video'], generate.id);
      });
      const at = await roomAt(page);
      await carry(page, row, at);
      flow.shot_dropped_at = at;
      await expect.poll(async () => {
        SHOT = (await project.assets()).find((asset) => !before.has(asset.name) && asset.tool === generate.id)?.name ?? null;
        return SHOT;
      }, { message: `the Shot box made, let go at ${JSON.stringify(at)}`, timeout: 60_000 }).not.toBeNull().catch(async (error) => {
        failed('the boxes when the Shot made none', (await project.assets()).map((asset) => `${asset.name}: ${asset.tool || asset.kind}`).join(', '));
        throw error;
      });
      await expect.poll(() => canvasVisible(page, SHOT), { timeout: 60_000 }).toBe(true);
      const r = await look(SHOT);
      await r.getByRole('textbox', { name: 'Summary', exact: true }).click();
      await page.keyboard.type(SUMMARY);
      await typeBeat(r, 0, BEAT);
      await expect.poll(async () => (await project.asset(SHOT))?.fields?.beats?.[0]?.description, { message: 'its beat saved', timeout: 30_000 }).toBe(BEAT);
      const fields = (await project.asset(SHOT)).fields;
      flow.shot = { name: SHOT, fields };
      expect.soft(fields.summary, 'its summary saved').toBe(SUMMARY);
      expect.soft(fields.beats[0].duration ?? 15, "Shot's beat: 15 s by default").toBe(15);
      await shot('shot-typed');
      await leave(page);
      mark('shot made', { name: SHOT });
    });

    await test.step("H3's Edit from Tools › Video dropped on the plate: VFX edit, its beat as long as the plate", async () => {
      await fit(page);
      const row = await toolRow(['Video', 'Edit Video'], edit.id);
      await carry(page, row, await canvasPoint(page, 'plate'));
      await expect.poll(async () => (await project.asset('plate'))?.tool ?? null, { message: 'the plate takes the Edit tool', timeout: 60_000 }).toBe(edit.id);
      const r = await look('plate');
      await draft(r);
      await choose(r, 'Edit', 'VFX edit');
      await typeBeat(r, 0, GUIDE);
      // Its beat as long as the plate, where the page offers a beat's length; an edit is as long as its plate either way.
      const duration = r.getByRole('slider', { name: "Beat 1's duration", exact: true });
      if (await duration.isVisible()) {
        await duration.focus();
        await page.keyboard.press('Home');
        for (let s = 1; s < PLATE_S; s++) await page.keyboard.press('ArrowRight');
        await expect.poll(async () => (await project.asset('plate'))?.fields?.beats?.[0]?.duration, { message: `its beat ${PLATE_S} s`, timeout: 30_000 }).toBe(PLATE_S);
      }
      await expect.poll(async () => (await project.asset('plate'))?.fields?.edit, { message: 'VFX edit chosen', timeout: 30_000 }).toBe('vfx_edit');
      flow.edit = { name: 'plate', fields: (await project.asset('plate')).fields };
      await shot('edit-typed');
      await leave(page);
      mark('edit made');
    });

    // Every wait from here is on the page's own stream: heard before anything is bought, or nothing is (red 43866620725 bought A and
    // heard nothing: the page passes its fetch a URL object).
    await expect.poll(() => heard, { message: "the page's event stream, as the page reads it", timeout: 60_000 }).toBeGreaterThan(0);
    mark('stream heard', { events: heard });
    const A = await test.step(`machine A rented through the page: ${SKU}, one GPU`, async () => {
      const m = await order('A', SKU, 1);
      return RENT ? ready(m) : m;
    });
    if (!RENT) {
      await test.step(`machine B quoted through the page: ${SKU_B}, ${GPUS_B} GPUs`, () => order('B', SKU_B, GPUS_B));
      mark('pre-flight: nothing ordered');
      return;
    }

    // cl-1104 (Paul, 11-16): "when a user attaches a tool … we could figure out what machine, if we got a request, that request would
    // land on. On that machine, we can pre-emptively install packages + models". With A up, the Shot's box says so beside its Render
    // once A has H3 and its weights; request 1 goes then, and downloads and installs nothing (the judge's line). The wait is bounded
    // well under a rental's 15 idle minutes (frozen-RC 44000183889 waited 120 min, A released itself and nothing rendered): not
    // Ready by then, the box's words are kept, the judge's line fails, and Render goes all the same so every other line runs.
    await test.step("the Shot's box says it's ready on A before its Render", async () => {
      const r = await look(SHOT);
      const note = r.locator('.ready-on');
      const began = Date.now();
      let said = '';
      while (Date.now() - began < READY_MS) {
        said = ((await note.first().textContent({ timeout: 1000 }).catch(() => '')) ?? '').trim();
        if (said.startsWith('Ready on ')) break;
        await page.waitForTimeout(2000);
      }
      const ready = said.startsWith('Ready on ');
      // Beside its Render, what the box says instead: its render time, Slow here, or nothing.
      const beside = ready ? said : (await r.locator('.ready-on, .render-time, .slow-here').allTextContents().catch(() => [])).map((t) => t.trim()).filter(Boolean).join(' · ');
      flow.ready_on = { box: SHOT, ready, said: beside, machine: A.name, at: Date.now(), waited_s: Math.round((Date.now() - began) / 1000) };
      keep();
      if (!ready) throw new Error(`no "Ready on" in ${READY_MS / 60_000} min; beside Render: ${JSON.stringify(beside)}`);
      mark('the Shot says ready', { said });
      await shot('ready-on-A');
    }).catch((error) => failed("the Shot's box says it's ready on A before its Render", error));

    await test.step('request 1: the Shot rendered, on A', async () => {
      const first = await render(1, SHOT, GENERATE);
      await shot('request-1-sent');
      await until('request 1 on A', () => (runs.get(first.id)?.machine ? true : null));
    });

    await test.step('request 2: the plate edited, behind it on A', async () => {
      await render(2, 'plate', EDIT);
      await shot('request-2-sent');
      await look(SHOT);
    });

    // Without B the flow goes on: requests 3 and 4 are placed on what there is, and the judge says what B didn't do.
    const B = await test.step(`machine B ordered through the page: ${SKU_B}, ${GPUS_B} GPUs, preferred as it boots`, () => order('B', SKU_B, GPUS_B))
      .catch((error) => { failed(`machine B ordered through the page: ${SKU_B}, ${GPUS_B} GPUs`, error); return flow.machines.B; });
    await goto(project.url());
    await expect(page.locator('.stage')).toBeVisible({ timeout: 60_000 });

    await test.step('request 3: the Shot again, while B boots and A is busy', async () => {
      await render(3, SHOT, GENERATE);
      await shot('request-3-sent');
    });

    await test.step('B ready, then request 4: the Shot once more, with both machines busy', async () => {
      if (B?.ordered_at) await ready(B).catch((error) => failed('B ready', error));
      await render(4, SHOT, GENERATE);
      await shot('request-4-sent');
    });

    await test.step('every take lands in the project and shows', async () => {
      // Focus stays on the Shot (two takes, one per machine when placed well) until both land, then on the plate's edit.
      const takes = async (name) => (await project.asset(name))?.versions ?? 0;
      // A take for each run that completed, beside the plate's own file: a run that failed lands none, and nothing waits for one.
      const held = { [SHOT]: 0, plate: 1 };
      for (const name of [SHOT, 'plate']) {
        await look(name);
        const ended = await until(`${name}'s takes`, () => {
          const mineNow = flow.sent.filter((sent) => sent.box === name && sent.run).map((sent) => runs.get(sent.run));
          return mineNow.every((run) => run && settled(run)) ? mineNow : null;
        });
        await shot(`${name}-settled`);
        const made = ended.filter((run) => run.status === 'completed').length;
        try {
          if (made < ended.length) throw new Error(`${made} of its ${ended.length} runs completed: ${ended.map((run) => `${run.name} ${run.status}${run.reason?.code ? ` (${run.reason.code})` : ''}`).join(', ')}`);
          await expect.poll(() => takes(name), { message: `${name}'s takes landed`, timeout: 900_000, intervals: [5_000] }).toBeGreaterThanOrEqual(held[name] + made);
          mark(`${name}'s takes landed`);
        } catch (error) {
          failed(`${name}'s takes landed`, error);
        }
      }
    });

    await test.step("the Shot's take in Focus: the video at its own shape, not a thumbnail", async () => {
      const r = await look(SHOT);
      await expect(r.locator('.focus-image-transform > .media')).toBeVisible({ timeout: 60_000 });
      const poster = await r.locator('.media-pixels .scrub img').first().evaluate((img) => ({ natural: [img.naturalWidth, img.naturalHeight], shown: [img.getBoundingClientRect().width, img.getBoundingClientRect().height] })).catch(() => null);
      await r.locator('.tl-transport').getByRole('button', { name: 'Play', exact: true }).click();
      const live = r.locator('video.live');
      await expect.poll(async () => (await live.count()) && live.evaluate((el) => el.currentTime), { message: 'it plays', timeout: 60_000 }).toBeGreaterThan(0.2);
      const video = await live.evaluate((el) => ({ natural: [el.videoWidth, el.videoHeight], shown: [el.getBoundingClientRect().width, el.getBoundingClientRect().height] }));
      const frame = await r.evaluate((el) => [el.getBoundingClientRect().width, el.getBoundingClientRect().height]);
      flow.focus = { box: SHOT, poster, video, region: frame };
      await shot('shot-in-focus');
      await r.locator('.tl-transport').getByRole('button', { name: 'Pause', exact: true }).click().catch(() => {});
      await look('plate');
      await shot('edit-in-focus');
      await leave(page);
    });
    flow.machines.A = A;
    flow.machines.B = B;
  } catch (error) {
    failed('the flow', error);
    await shot('failed');
    throw error;
  } finally {
    try { flow.calcifer.pid_at_end = holder().pid; } catch (error) { flow.calcifer.pid_at_end = String(error); }
    if (RENT) {
      await test.step("what's needed to judge it: each run's record, each machine's record and TensorD log, Calcifer's log", async () => {
        const save = (file, ...words) => new Promise((done) => {
          const out = openSync(join(FLOW, file), 'w');
          const child = spawn(need('COZY'), words, { cwd: project.path, stdio: ['ignore', out, out] });
          child.on('exit', (code) => { closeSync(out); done(code); });
        });
        for (const run of callRuns()) await save(`runs/${run.name || run.number}.json`, 'run', 'watch', run.name || String(run.number), '--once', '--json', '--full');
        await save('runs.json', '--json', 'run', 'ls', '--limit', '0');
        for (const m of Object.values(flow.machines).filter((one) => one.name)) {
          await save(`machines/${m.label}.json`, '--json', 'machine', 'show', m.name);
          await save(`machines/${m.label}-tensord.log`, 'machine', 'logs', m.name, 'tensord', '--since', '6h', '-n', '0');
        }
        await save('calcifer.log', 'machine', 'logs', 'local', 'calcifer', '--lines', '20000');
        // Where its walk placed each take and why, each machine's quote (calcifer.diagnostics, the session's): kept in the project's own
        // readable view, never in Calcifer's log.
        const { view } = await api.get(project.at('/vault')).catch(() => ({}));
        try { copyFileSync(join(view ?? project.path, '.vault-calcifer', 'diagnostics', 'walk.log'), join(FLOW, 'walk.log')); } catch (error) { failed('the walk log', error); }
        mark('collected');
      }).catch((error) => failed('collection', error));
      await test.step('both machines ended through the page', async () => {
        for (const m of Object.values(flow.machines).filter((one) => one.name || one.rental_id)) {
          try {
            // Gone from the list, or ending, before End is pressed: it ended by itself (idle 15 min). Said with when; the judge says why.
            const now = machineOf(m);
            if (!now || ['ending', 'ended'].includes(now.state)) {
              Object.assign(m, { left_at: m.left_at ?? Date.now(), left_state: now?.state ?? m.left_state ?? null });
              continue;
            }
            delete m.left_at;
            delete m.left_state;
            await goto(`${project.url()}/machines/${encodeURIComponent(m.name ?? m.rental_id)}`);
            const actions = page.locator('.machine-actions');
            await actions.getByRole('button', { name: 'End', exact: true }).click({ timeout: 60_000 });
            const confirm = page.locator('.confirm');
            await expect(confirm).toBeVisible();
            m.end_says = (await confirm.locator('.says').textContent().catch(() => '')).trim();
            // "End rental" since cl-1084's words; "End it" before (tonight's RC).
            await confirm.getByRole('button', { name: /^(End rental|End it)$/ }).click();
            m.end_pressed_at = Date.now();
            await until(`${m.label} ended`, () => (!machineOf(m) || machineOf(m).state === 'ended' ? true : null));
            m.ended_at = Date.now();
            await shot(`${m.label}-ended`);
          } catch (error) {
            failed(`${m.label} ended through the page`, error);
          }
        }
      });
    }
    await cli('--json', 'vault', '--directory', project.path, 'lock').catch(() => {});
    keep();
  }
});
