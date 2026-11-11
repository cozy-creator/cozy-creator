// The top bar's Run on (cl-873): a project pinned to machines from the page, kept in the project rather than the browser, so the CLI
// reads the same. Render all sends its boxes only to them: each run on one of the two, none on the third machine automatic placement would
// also take. `cozy machine use` shows on the open page within seconds, a reload keeps it, and Automatically is no machine at all.
//   node runon.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $STANDS: two stand-in machines added there
// (machineset.sh), the first pinned with local and the second left out. $PLAYWRIGHT, $CHROME, $COZY, $SHOT as nodes.js. Prints what it saw
// as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { join } = require('path');
const { calcifer, cozyIn, lastLook, launch, lookAt, noStall, projectAction, projectFromPage, signIn, STEP_MS, until, watchBoxes } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const finish = (code) => {
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(code);
};
const TOOL = join(__dirname, '..', 'project', 'tool.py');
const [IN, OUT] = (process.env.STANDS ?? '').split(' ');
const RENDER_MS = 420_000;
const same = (a, b) => JSON.stringify(a ?? []) === JSON.stringify(b);

(async () => {
  if (!IN || !OUT) {
    check('stand_ins', false, process.env.STANDS ?? 'none');
    return finish(1);
  }
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1100 } })).newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}` } })).json();

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-runon-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  const names = ['runon_a', 'runon_b', 'runon_c', 'runon_d'];
  for (const [i, name] of names.entries()) await projectAction(folder, 'add', `${TOOL}/still`, name, `at=${200 + i * 300},400,260`, `instructions=Run on, ${name}`);

  // Pinned from the top bar, one machine at a time, each kept by Calcifer.
  const summary = page.locator('.run-on > summary');
  const said = () => summary.textContent().catch(() => '');
  await until('Run on in the top bar', said, (text) => /Run on/.test(text));
  check('starts_automatic', /Automatically/.test(await said()), await said());
  await summary.click();
  await page.locator('.run-on input[value="local"]').check();
  await until('local kept', state, (now) => same(now.machines, ['local']));
  await page.locator(`.run-on input[value="${IN}"]`).check();
  const pinned = await until('both kept', state, (now) => same(now.machines, ['local', IN])).catch(() => null);
  check('pinned_in_the_project', pinned !== null, pinned?.machines);
  await shot('pinned');
  await summary.click();
  check('summary_names_them', (await said()).includes('This computer') && (await said()).includes(IN), await said());
  const status = JSON.parse(cozyIn(folder, ['--json', 'status'], { encoding: 'utf8' }));
  check('cli_reads_the_same', same(status.project?.machines, ['local', IN]), status.project);
  check('nothing_in_the_browser', await page.evaluate(() => Object.keys(localStorage).every((key) => !key.startsWith('cozy.machine'))));

  // Render all: every box made, each run on local or the first stand-in, none on the second.
  await page.locator('.render-all button.primary').click();
  const settled = (held) => held && (held.versions >= 1 || held.failure);
  const made = await until('every box made or failed', state, (now) => !now.busy && names.every((name) => settled(now.assets.find((held) => held.name === name))), RENDER_MS);
  seen.failed = made.assets.filter((held) => names.includes(held.name) && held.failure).map((held) => `${held.name}: ${held.failure.message}`);
  const runs = ((await api(`/v1/runs?project=${id}&limit=100`)).runs ?? []).filter((run) => /\/still(@\S+)?$/.test(run.target ?? ''));
  seen.placed = runs.map((run) => `${run.number} ${run.machine} ${run.status}`);
  check('rendered', runs.length >= names.length && runs.every((run) => run.status === 'completed'), seen.placed);
  check('inside_the_set', runs.every((run) => run.machine === 'local' || run.machine === IN), seen.placed);
  check('none_left_out', !runs.some((run) => run.machine === OUT), seen.placed);

  // The CLI's choice on the open page within seconds, kept across a reload.
  cozyIn(folder, ['machine', 'use', OUT]);
  const live = await until('the CLI\'s choice on the page', said, (text) => text.includes(OUT) && !text.includes('This computer'), 15_000).catch(() => null);
  check('cli_choice_live', live !== null, await said());
  await page.reload();
  await page.waitForSelector('.run-on > summary');
  await until('Run on after the reload', said, (text) => text.includes(OUT), STEP_MS).catch(() => {});
  check('kept_across_reload', (await said()).includes(OUT), await said());

  // Automatically: no machine at all.
  await summary.click();
  await page.locator('.run-on label', { hasText: 'Automatically' }).locator('input').check();
  const any = await until('automatic kept', state, (now) => same(now.machines, [])).catch(() => null);
  check('automatic_is_none', any !== null && /Automatically/.test(await said()), await said());
  await shot('automatic');

  check('no_page_errors', errors.length === 0, errors);
  check('ran', true);
  await browser.close();
  finish(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
