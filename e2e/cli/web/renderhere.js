// Render here (cl-887): a project's machines (cl-873) chosen in the machines panel. A machine's Render here adds it to the open project's
// machines and pressed again takes it off; each is green wherever the panel lists it (its row, its drawer, its chip top right), and none
// green says once that renders are placed automatically. Render all sends its boxes only to the green ones: each run on one of the two,
// none on the third machine automatic placement would also take. `cozy machine use` turns the green on the open page within seconds, a
// reload keeps it, and taking the last one off is automatic again, no machine at all.
//   node renderhere.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $STANDS: two stand-in machines added
// there (machineset.sh), the first pinned with local and the second left out. $PLAYWRIGHT, $CHROME, $COZY, $SHOT as nodes.js. Prints what
// it saw as JSON, exits 1 if anything failed.
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
const AUTOMATIC = /No machine is green, so this project's renders are placed automatically/;

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
  await page.fill('input[aria-label="New project\'s name"]', `web-renderhere-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const at = page.url();
  const id = (await projectFromPage(page)).id;
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  const names = ['here_a', 'here_b', 'here_c', 'here_d'];
  for (const [i, name] of names.entries()) await projectAction(folder, 'add', `${TOOL}/still`, name, `at=${200 + i * 300},400,260`, `instructions=Render here, ${name}`);

  // What the panel shows green: its rows, and the chips top right.
  const green = async () => ({
    rows: await page.locator('.drawer .runrow.render-here').evaluateAll((all) => all.map((row) => row.dataset.machine).sort()),
    chips: await page.locator('.mtab.render-here').evaluateAll((all) => all.map((chip) => chip.dataset.machine).sort()),
  });
  const greenAre = (want) => (now) => same(now.rows, [...want].sort()) && same(now.chips, [...want].sort());
  const said = () => page.locator('.drawer').innerText().catch(() => '');
  const list = async () => {
    if (!page.url().endsWith('/machines')) await page.goto(`${at}/machines`);
    await page.locator('.drawer .runrow').first().waitFor();
  };
  // A machine's drawer from its row, and its Render here pressed.
  const press = async (name) => {
    await list();
    await page.locator(`.drawer .runrow[data-machine="${name}"] a`).first().click();
    const button = page.locator('.drawer button.render-here');
    await button.waitFor();
    const was = await button.getAttribute('aria-pressed');
    await button.click();
    await until(`${name}'s Render here turned`, () => button.getAttribute('aria-pressed'), (now) => now !== was, 5000).catch(() => {});
    const pressed = await button.getAttribute('aria-pressed');
    const head = await page.locator('.drawer .run-head.render-here').count();
    await page.locator('.drawer a[aria-label="Back"]').click();
    await page.waitForURL(/\/machines$/);
    return { pressed, head };
  };

  // None green: automatic, said once.
  await list();
  const atFirst = await green();
  check('starts_automatic', atFirst.rows.length === 0 && atFirst.chips.length === 0 && (await said()).match(new RegExp(AUTOMATIC, 'g'))?.length === 1, { atFirst, said: await said() });
  check('no_run_on_menu', (await page.locator('.run-on, [aria-label="Render machines"]').count()) === 0);

  // Render here on local, then on the first stand-in: each kept by Calcifer, each green in the list, its drawer and its chip.
  const local = await press('local');
  await until('local kept', state, (now) => same(now.machines, ['local']));
  const first = await press(IN);
  const pinned = await until('both kept', state, (now) => same(now.machines, ['local', IN])).catch(() => null);
  check('pressed_shows_pressed', local.pressed === 'true' && local.head === 1 && first.pressed === 'true' && first.head === 1, { local, first });
  check('pinned_in_the_project', pinned !== null, pinned?.machines);
  const both = await until('both green', green, greenAre(['local', IN])).catch(() => null);
  check('green_where_listed', both !== null, await green());
  check('automatic_not_said', !AUTOMATIC.test(await said()), await said());
  await shot('green');
  const status = JSON.parse(cozyIn(folder, ['--json', 'status'], { encoding: 'utf8' }));
  check('cli_reads_the_same', same(status.project?.machines, ['local', IN]), status.project);
  check('nothing_in_the_browser', await page.evaluate(() => Object.keys(localStorage).every((key) => !key.startsWith('cozy.machine'))));

  // Render all: every box made, each run on local or the first stand-in, none on the second.
  await page.goto(at);
  await page.locator('.render-all button.primary').click();
  const settled = (held) => held && (held.versions >= 1 || held.failure);
  const made = await until('every box made or failed', state, (now) => !now.busy && names.every((name) => settled(now.assets.find((held) => held.name === name))), RENDER_MS);
  seen.failed = made.assets.filter((held) => names.includes(held.name) && held.failure).map((held) => `${held.name}: ${held.failure.message}`);
  const runs = ((await api(`/v1/runs?project=${id}&limit=100`)).runs ?? []).filter((run) => /\/still(@\S+)?$/.test(run.target ?? ''));
  seen.placed = runs.map((run) => `${run.number} ${run.machine} ${run.status}`);
  check('rendered', runs.length >= names.length && runs.every((run) => run.status === 'completed'), seen.placed);
  check('inside_the_set', runs.every((run) => run.machine === 'local' || run.machine === IN), seen.placed);
  check('none_left_out', !runs.some((run) => run.machine === OUT), seen.placed);

  // The CLI's choice green on the open page within seconds, kept across a reload.
  await list();
  cozyIn(folder, ['machine', 'use', OUT]);
  const live = await until('the CLI\'s choice green', green, greenAre([OUT]), 15_000).catch(() => null);
  check('cli_choice_live', live !== null, await green());
  await page.reload();
  await list();
  const kept = await until('green after the reload', green, greenAre([OUT])).catch(() => null);
  check('kept_across_reload', kept !== null, await green());

  // Pressed again, the last one comes off: no machine at all, automatic, said again.
  const off = await press(OUT);
  const any = await until('automatic kept', state, (now) => same(now.machines, [])).catch(() => null);
  const none = await until('none green', green, greenAre([])).catch(() => null);
  check('pressed_again_takes_it_off', off.pressed === 'false' && any !== null && none !== null, { off, machines: (await state()).machines, green: await green() });
  check('automatic_said_again', AUTOMATIC.test(await said()), await said());
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
