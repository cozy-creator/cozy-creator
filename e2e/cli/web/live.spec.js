// live (cl-973 core cell 16): the canvas, open and never reloaded, shows each change the CLI (`cozy apply`) and an API client make within
// 1.2 s of its command returning, and keeps words being typed meanwhile; Runs shows the CLI's run live to its end with its log, and Cancel
// asks inline before it acts; Machines lists what `cozy machine ls` does; tabs share one `GET /v1/events`, which the second takes on when
// the first closes. The CLI has no verb that renames or moves a box (a node file holds kind, description, text, approved, takes and rev):
// those two are the API's.
const { mkdirSync, writeFileSync } = require('fs');
const { test, expect, box, canvasIdle, focus, fixtureTool, png } = require('./fixtures');

/** Polls whose miss leaves the cell going, as `expect.soft` does. */
const softly = expect.configure({ soft: true });

/** From a command's return to the canvas showing it ("about a second"). */
const LIVE_MS = 1200;
const SLOW = fixtureTool('tool.py/slow');

/** In the page: `window.__shown(spec, ms)` resolves with the time of the first frame `spec` holds on the canvas, or null after `ms`. */
function watchCanvas() {
  const card = (name) => document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(name)}"]`);
  const holds = (spec) => {
    if (spec.all) return spec.all.every(holds);
    if (spec.gone) return !card(spec.gone);
    if (spec.focused) return Boolean(document.querySelector(`[data-focused-asset="${CSS.escape(spec.focused)}"]`));
    const el = card(spec.name);
    if (!el) return false;
    if (spec.text !== undefined && !el.textContent.includes(spec.text)) return false;
    if (spec.label !== undefined && el.querySelector('.node-version-label')?.textContent !== spec.label) return false;
    if (spec.at) {
      const { e, f } = new DOMMatrix(getComputedStyle(el.closest('.react-flow__node')).transform);
      if (Math.abs(e - spec.at.x) > 0.5 || Math.abs(f - spec.at.y) > 0.5) return false;
    }
    return true;
  };
  window.__shown = (spec, ms) => new Promise((done) => {
    const began = performance.now();
    const look = () => (holds(spec) ? done(Date.now()) : performance.now() - began > ms ? done(null) : requestAnimationFrame(look));
    look();
  });
  window.notReloaded = true;
}

/** `act`, then how long from its return until `spec` holds on the canvas: within LIVE_MS. */
async function timed(page, label, spec, act) {
  const shown = page.evaluate(([spec, ms]) => window.__shown(spec, ms), [spec, 15_000]);
  await act();
  const returned = Date.now();
  const when = await shown;
  expect.soft(when === null ? Infinity : when - returned, `${label}: ms from its command returning to the canvas showing it`).toBeLessThanOrEqual(LIVE_MS);
}

/** Each `method` call to `path` waits here, unsent, until `release`: as a slow network holds it. */
async function hold(page, method, path) {
  const calls = [];
  let open;
  const held = new Promise((done) => (open = done));
  const matches = (url) => url.pathname === path;
  const handler = async (route) => {
    if (route.request().method() !== method) return route.fallback();
    calls.push(route.request());
    await held;
    await route.fallback();
  };
  await page.route(matches, handler);
  return { calls, release: async () => { open(); await page.unroute(matches, handler); } };
}

/** `cozy apply -f` of `nodes`, written as a file of the cell's own. */
async function apply(cli, file, nodes) {
  writeFileSync(file, JSON.stringify(nodes));
  const applied = await cli.json('apply', '-f', file);
  expect(applied.applied, `cozy apply ${JSON.stringify(nodes)}: ${JSON.stringify(applied.errors ?? applied.conflicts)}`).toBe(true);
}

/** Its rev, as `cozy get` says it: what an edit of it merges against. */
const revOf = async (cli, name) => (await cli.json('get', name)).rev;

/** This project's call runs, by number. */
const callRuns = async (api, project) => (await api.get(project.at('/runs?limit=50'))).runs.filter((run) => run.kind === 'call');

/** The run `started` began: one this project didn't have in `before`. */
async function newRun(api, project, before) {
  let run;
  await expect.poll(async () => {
    run = (await callRuns(api, project)).find((one) => !before.includes(one.number));
    return run?.number;
  }, { message: 'the CLI run is in the project', timeout: 60_000 }).toBeGreaterThan(0);
  return run;
}

test('live', async ({ page, context, project, api, cli, signIn, guards }, testInfo) => {
  mkdirSync(testInfo.outputDir, { recursive: true });
  const file = (name) => testInfo.outputPath(name);
  await page.evaluate(watchCanvas);
  const card = (name) => box(page, name);

  await test.step('the CLI and an API client add, set words, import, rename, approve and move: each live on the open canvas', async () => {
    const nodes = file('nodes.json');
    // The CLI, in the project's folder.
    await timed(page, 'cozy apply: a text node', { name: 'cli_a', text: 'Words from the CLI' }, () => apply(cli, nodes, { cli_a: { kind: 'text', text: 'Words from the CLI' } }));
    let rev = await revOf(cli, 'cli_a');
    await timed(page, 'cozy apply: its words', { name: 'cli_a', text: 'Second CLI words' }, () => apply(cli, nodes, { cli_a: { rev, text: 'Second CLI words' } }));
    writeFileSync(file('one.png'), png(30));
    writeFileSync(file('two.png'), png(90));
    await timed(page, 'cozy apply: a picture imported, two takes', { name: 'cli_pic' }, () => apply(cli, nodes, { cli_pic: { kind: 'image', takes: [{ file: file('one.png') }, { file: file('two.png') }] } }));
    // Each shows the version that is its state; the other is approved.
    let was = (await project.asset('cli_pic')).version;
    await expect.soft(card('cli_pic').locator('.node-version-label'), 'cli_pic shows the version that is its state').toHaveText(`v${was}`);
    let other = was === 2 ? 1 : 2;
    rev = await revOf(cli, 'cli_pic');
    await timed(page, `cozy apply: take ${other} approved`, { name: 'cli_pic', label: `v${other}` }, () => apply(cli, nodes, { cli_pic: { rev, approved: other } }));
    expect.soft((await project.asset('cli_pic')).approved, 'cozy apply approved it').toBe(other);

    // An API client, as another program writes.
    await timed(page, 'API: a text box', { name: 'api_a', text: 'Words from the API' }, () => project.add({ name: 'api_a', kind: 'text', fields: { text: 'Words from the API' } }));
    await timed(page, 'API: its words', { name: 'api_a', text: 'Second API words' }, () => api.patch(project.at('/assets/api_a'), { fields: { text: 'Second API words' } }));
    await timed(page, 'API: a picture imported', { name: 'api_pic' }, () => project.import(png(150), { name: 'api_pic' }));
    await api.post(project.at('/assets/api_pic/versions'), { source: await project.upload(png(210)) });
    await timed(page, 'API: a rename', { all: [{ name: 'api_b', text: 'Second API words' }, { gone: 'api_a' }] }, () => api.post(project.at('/rename'), { from: 'api_a', to: 'api_b' }));
    was = (await project.asset('api_pic')).version;
    await expect.soft(card('api_pic').locator('.node-version-label'), 'api_pic shows the version that is its state').toHaveText(`v${was}`);
    other = was === 2 ? 1 : 2;
    await timed(page, `API: take ${other} approved`, { name: 'api_pic', label: `v${other}` }, () => api.post(project.at('/approve'), { name: 'api_pic', version: String(other) }));
    expect.soft((await project.asset('api_pic')).approved, 'the API approved it').toBe(other);
    // Somewhere clear, right of every box: one let go on another's room gives way, by the page's own rule, which no-overlap's to judge.
    const { cards } = await api.get(project.at('/canvas'));
    const right = Math.max(...Object.values(cards).map((held) => held.x + (held.w ?? 240)));
    const to = { x: Math.round(right + 160), y: Math.round(cards.api_b.y + 40) };
    await timed(page, 'API: a move', { name: 'api_b', at: to }, () => api.patch(project.at('/canvas'), { cards: { api_b: to } }));

    // Words being typed in cli_a meanwhile, at a person's pace: the CLI sets its description, and the API renames the box while the page's
    // read of that is still on its way, so the read finds nothing by the old name (the 404 is this cell's doing). The page follows the
    // rename: Focus stays on the box under its new name, and every typed character stays and is saved there beside the CLI's description.
    guards.allow(/status of 404\b.*\/v1\/projects\/[^/ ]+\/assets\/cli_a$/);
    const notFound = [];
    page.on('response', (answer) => {
      if (answer.status() === 404 && new URL(answer.url()).pathname === project.at('/assets/cli_a')) notFound.push(answer.request().method());
    });
    const region = await focus(page, 'cli_a');
    const words = region.getByRole('textbox', { name: 'Text', exact: true });
    await words.click();
    await page.keyboard.press('ControlOrMeta+End');
    const typing = page.keyboard.type(' TYPED-MEANWHILE', { delay: 150 });
    const reads = await hold(page, 'GET', project.at('/assets/cli_a'));
    rev = await revOf(cli, 'cli_a');
    await apply(cli, nodes, { cli_a: { rev, description: 'Said by the CLI meanwhile' } });
    await softly.poll(() => reads.calls.length, { message: "the page reads the CLI's description" }).toBeGreaterThan(0);
    await timed(page, 'API: a rename of the box being typed in, its read on the way', { focused: 'cli_typed' }, async () => {
      await api.post(project.at('/rename'), { from: 'cli_a', to: 'cli_typed' });
      await reads.release();
    });
    await typing;
    const open = page.getByRole('region', { name: 'Focused asset' });
    await expect.soft(open.getByRole('textbox', { name: 'Text', exact: true }), 'what was typed, kept through both').toHaveText('Second CLI words TYPED-MEANWHILE');
    // Back to the canvas from the keyboard, where the hands are.
    await page.keyboard.press('Escape');
    await canvasIdle(page);
    await softly.poll(async () => (await project.asset('cli_typed'))?.fields, { message: "the typed words saved beside the CLI's description" })
      .toMatchObject({ text: 'Second CLI words TYPED-MEANWHILE', description: 'Said by the CLI meanwhile' });
    expect.soft(notFound.filter((method) => method === 'GET').length, 'the reads held past its rename found nothing by its old name').toBeGreaterThanOrEqual(reads.calls.length);
    expect.soft(notFound.filter((method) => method !== 'GET'), 'no write went under its old name').toEqual([]);
    expect.soft(await page.evaluate(() => window.notReloaded), 'the canvas was never reloaded').toBe(true);
  });

  await test.step('cozy run a fixture tool, then Cancel a run', async () => {
    const drawer = page.locator('aside.drawer');
    const head = drawer.locator('.run-head');
    const menu = async (name) => {
      await page.locator('details.main-menu > summary').click();
      await page.locator('details.main-menu nav').getByRole('link', { name }).click();
    };
    await menu(/^Runs/);
    await expect(drawer.locator('.drawer-head')).toContainText('Runs');
    const rowOf = (number) => drawer.locator('li.runrow').filter({ has: page.locator('b', { hasText: new RegExp(`^#${number}$`) }) });
    const log = drawer.locator('section.sect', { has: page.getByRole('heading', { name: 'Log', exact: true }) }).locator('pre.log');

    // A real run the terminal asks for, followed by the CLI to its end: listed and followed live, with no reload.
    let before = (await callRuns(api, project)).map((run) => run.number);
    const first = cli('run', SLOW, 'seconds:=10', '--on', 'local');
    const run = await newRun(api, project, before);
    await expect(rowOf(run.number), 'the CLI run listed live').toBeVisible({ timeout: 30_000 });
    await rowOf(run.number).getByRole('link').click();
    await expect(drawer.locator('.drawer-head')).toContainText(`Run ${run.number}`);
    await expect(head).toContainText('running', { timeout: 120_000 });
    await expect.soft(head, 'its end, live').toContainText('completed', { timeout: 120_000 });
    await expect.soft(log, 'its log, to its end').toContainText('completed');
    expect.soft((await first).code, 'the CLI followed it to its end').toBe(0);
    expect.soft((await api.get(project.at(`/runs/${encodeURIComponent(run.id)}`))).status).toBe('completed');

    // Another, canceled from the page: asked inline first, then done.
    before = (await callRuns(api, project)).map((one) => one.number);
    const second = cli('run', SLOW, 'seconds:=300', '--on', 'local');
    const long = await newRun(api, project, before);
    const status = async () => api.get(project.at(`/runs/${encodeURIComponent(long.id)}`));
    try {
      await drawer.getByRole('link', { name: 'Back' }).click();
      await expect(rowOf(long.number)).toBeVisible({ timeout: 30_000 });
      await rowOf(long.number).getByRole('link').click();
      await expect(head).toContainText('running', { timeout: 120_000 });
      const cancel = drawer.locator('.drawer-head').getByRole('button', { name: 'Cancel', exact: true });
      const asking = drawer.locator('.confirm');
      await cancel.click();
      await expect.soft(cancel).toHaveAttribute('aria-expanded', 'true');
      await expect.soft(asking.locator('.says'), 'Cancel asks there, inline').toHaveText(`Cancel run ${long.number}? What it hasn't finished is lost. Its machine goes on.`);
      expect.soft(await status(), 'nothing done before its yes').toMatchObject({ status: 'in_progress', cancel_requested: false });
      await asking.getByRole('button', { name: 'Not now' }).click();
      await expect.soft(asking).toHaveCount(0);
      await expect.soft(head).toContainText('running');
      await cancel.click();
      await asking.getByRole('button', { name: 'Cancel it' }).click();
      await expect.soft(head, 'canceled, live').toContainText('canceled', { timeout: 60_000 });
      await softly.poll(async () => (await status()).status, { message: 'the run canceled' }).toBe('canceled');
      await second;
    } finally {
      // A run left going holds this job's local machine for the cells after.
      if (!['completed', 'failed', 'canceled'].includes((await status()).status)) await api.post(project.at(`/runs/${encodeURIComponent(long.id)}/cancel`));
    }

    // Machines, as `cozy machine ls` lists them.
    await menu('Machines');
    await expect(drawer.locator('.drawer-head')).toContainText('Machines');
    const listed = (await cli.json('machine', 'ls')).machines.filter((machine) => machine.state !== 'ended').map((machine) => machine.name).sort();
    expect.soft(listed.length, 'cozy machine ls lists a machine').toBeGreaterThan(0);
    await softly.poll(async () => (await drawer.locator('li.runrow.machine').evaluateAll((rows) => rows.map((row) => row.dataset.machine))).sort(), { message: 'Machines lists what cozy machine ls does' }).toEqual(listed);
    expect.soft(await page.evaluate(() => window.notReloaded), 'never reloaded').toBe(true);
  });

  await test.step('a second tab shares the stream, and keeps it once the first closes', async () => {
    const streams = (tab) => {
      const opened = [];
      tab.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname === '/v1/events') opened.push(request); });
      return opened;
    };
    const tab = await context.newPage();
    const its = streams(tab);
    await signIn(tab, project.url());
    await expect(tab.locator('.stage')).toBeVisible();
    await tab.bringToFront();
    await project.add({ name: 'tab_a', kind: 'text', fields: { text: 'Seen in the second tab' } });
    await expect(box(tab, 'tab_a'), "the second tab hears the first tab's stream").toContainText('Seen in the second tab');
    expect.soft(its.length, 'the second tab opens no stream of its own').toBe(0);

    // The first closes: the second takes the stream on, says what it shows, and changes still arrive.
    const shown = tab.waitForResponse((answer) => answer.request().method() === 'PUT' && new URL(answer.url()).pathname.startsWith('/v1/events/'));
    await page.close();
    await softly.poll(() => its.length, { message: 'the second tab opens the one stream' }).toBe(1);
    expect.soft((await shown).ok(), 'it tells Calcifer what it shows').toBe(true);
    await api.patch(project.at('/assets/tab_a'), { fields: { text: 'After the first tab closed' } });
    await expect.soft(box(tab, 'tab_a'), 'changes keep arriving').toContainText('After the first tab closed');
    expect.soft(its.length, 'still one stream').toBe(1);
  });
});
