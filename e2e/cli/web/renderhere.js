// A project's preferred machines (cl-873, cl-887), a checkbox beside each machine (Paul 11-12: "Just make it a checkbox. When you checkbox a
// machine, it's highlighted in green, and you can highlight several"): at the start of each row of the machines panel, first in a chip's
// flyout top right, and in a machine's drawer. Checked, it's one of the open project's machines and green on its chip, its row and its
// drawer; nothing checked says once that renders are placed automatically. Every machine has a chip. Render all sends its boxes only to the green ones: each run on one of the two,
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
const AUTOMATIC = /Nothing checked: this project's renders are placed automatically/;

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

  // What shows green: the panel's rows, and the chips top right.
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
  /** Clicks `box` and waits for it to turn: whether it's checked now. */
  const turn = async (box, what) => {
    const was = await box.isChecked();
    await box.click();
    await until(what, () => box.isChecked(), (now) => now !== was, 5000).catch(() => {});
    return box.isChecked();
  };
  // A row's checkbox in the machines panel, at its start: nothing opens.
  const tick = async (name) => {
    await list();
    return turn(page.locator(`.drawer .runrow[data-machine="${name}"] > input.render-check`), `${name}'s row ticked`);
  };
  // A chip's flyout top right, its checkbox its first control.
  const tickChip = async (name) => {
    await page.locator(`.mtab[data-machine="${name}"]`).click();
    const box = page.locator('.activity input.render-check');
    await box.waitFor();
    seen.flyout_first_control = await page.locator('.activity input, .activity button, .activity a').first().evaluate((one) => one.classList.contains('render-check'));
    const checked = await turn(box, `${name}'s flyout ticked`);
    await page.keyboard.press('Escape');
    await page.locator('.activity').waitFor({ state: 'detached' });
    return checked;
  };

  // Nothing checked: automatic, said once; every machine has a chip.
  await list();
  const atFirst = await green();
  check('starts_automatic', atFirst.rows.length === 0 && atFirst.chips.length === 0 && (await said()).match(new RegExp(AUTOMATIC, 'g'))?.length === 1, { atFirst, said: await said() });
  check('no_run_on_menu', (await page.locator('.run-on, [aria-label="Render machines"], button.render-here').count()) === 0);
  const chips = await page.locator('.mtab').evaluateAll((all) => all.map((chip) => chip.dataset.machine));
  check('every_machine_has_a_chip', ['local', IN, OUT].every((name) => chips.includes(name)), chips);

  // Two ticked in the panel: both green, both the project's machines, as the CLI reads them too.
  const ticked = [await tick('local'), await tick(IN)];
  const pinned = await until('both kept', state, (now) => same(now.machines, ['local', IN])).catch(() => null);
  const both = await until('both green', green, greenAre(['local', IN])).catch(() => null);
  check('two_ticked_in_the_panel', ticked.every(Boolean) && pinned !== null && both !== null, { ticked, machines: (await state()).machines, green: await green() });
  check('automatic_not_said', !AUTOMATIC.test(await said()), await said());
  await shot('green');
  const status = JSON.parse(cozyIn(folder, ['--json', 'status'], { encoding: 'utf8' }));
  check('cli_reads_the_same', same(status.project?.machines, ['local', IN]), status.project);
  check('nothing_in_the_browser', await page.evaluate(() => Object.keys(localStorage).every((key) => !key.startsWith('cozy.machine'))));

  // Its drawer from the canvas, by its flyout's Details: its checkbox checked first on its face, green, and the rest behind Details.
  await page.goto(at);
  await page.locator(`.mtab[data-machine="${IN}"]`).click();
  await page.locator('.activity a', { hasText: 'Details' }).click();
  await page.locator('.drawer .run-head').waitFor();
  const face = await page.evaluate(() => {
    const drawer = document.querySelector('.drawer-body') ?? document.querySelector('.drawer');
    const first = drawer.querySelector('input, button');
    const details = [...drawer.querySelectorAll('details.sect')].find((held) => held.querySelector(':scope > summary')?.textContent === 'Details');
    return {
      first: first?.className, checked: first?.checked, green: Boolean(drawer.querySelector('.run-head.render-here')),
      doing: drawer.querySelector('.doing-now')?.textContent ?? null, details: Boolean(details), folded: details ? !details.open : null,
      kv_shown: [...drawer.querySelectorAll('.kv')].some((kv) => !kv.closest('details:not([open])')),
    };
  });
  check('drawer_from_the_canvas_leads_with_its_checkbox', face.first === 'render-check' && face.checked === true && face.green, face);
  check('drawer_face_is_slim', face.doing !== null && face.details && face.folded === true && !face.kv_shown, face);
  await shot('drawer');

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

  // Both unticked in the panel: no machine at all, automatic, said again.
  const unticked = [await tick('local'), await tick(IN)];
  const none = await until('automatic kept', state, (now) => same(now.machines, [])).catch(() => null);
  const plain = await until('none green', green, greenAre([])).catch(() => null);
  check('untick_both_is_automatic', unticked.every((one) => one === false) && none !== null && plain !== null && AUTOMATIC.test(await said()), { unticked, machines: (await state()).machines, green: await green() });
  await shot('automatic');

  // The CLI's choice checked and green on the open page within seconds, kept across a reload.
  await list();
  cozyIn(folder, ['machine', 'use', OUT]);
  const live = await until('the CLI\'s choice green', green, greenAre([OUT]), 15_000).catch(() => null);
  const box = page.locator(`.drawer .runrow[data-machine="${OUT}"] > input.render-check`);
  check('cli_choice_live', live !== null && (await box.isChecked()), await green());
  await page.reload();
  await list();
  const kept = await until('green after the reload', green, greenAre([OUT])).catch(() => null);
  check('kept_across_reload', kept !== null, await green());

  // In a chip's flyout top right, its checkbox first: one more ticked there, then both off there.
  const fromChip = await tickChip(IN);
  const twoNow = await until('the chip\'s tick kept', state, (now) => same(now.machines, [OUT, IN])).catch(() => null);
  const chipGreen = await until('its chip green', green, (now) => same(now.chips, [IN, OUT].sort())).catch(() => null);
  check('a_chip_ticks_it', fromChip === true && twoNow !== null && chipGreen !== null && seen.flyout_first_control === true, { fromChip, machines: (await state()).machines, green: await green(), first: seen.flyout_first_control });
  const offs = [await tickChip(OUT), await tickChip(IN)];
  const any = await until('automatic again', state, (now) => same(now.machines, [])).catch(() => null);
  check('chips_untick_to_automatic', offs.every((one) => one === false) && any !== null && AUTOMATIC.test(await said()), { offs, machines: (await state()).machines });

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
