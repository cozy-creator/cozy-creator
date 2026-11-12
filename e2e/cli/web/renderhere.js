// Render here (cl-887): a project's preferred machines (cl-873), chosen on the machine chips top right (Paul 11-12: "select it in the top
// right and then have a selection of machines that are preferred") and in a machine's drawer. Render here adds it to the open project's
// machines and pressed again takes it off; several can be on; each is green on its chip and wherever the machines panel lists it, every
// machine has a chip, and none green says once that renders are placed automatically. Render all sends its boxes only to the green ones: each run on one of the two,
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
  // A machine's chip top right: its flyout, and Render here, its first control, there (Paul 11-12).
  const pressChip = async (name) => {
    await page.locator(`.mtab[data-machine="${name}"]`).click();
    const button = page.locator('.activity button.render-here');
    await button.waitFor();
    seen.flyout_first_control = await page.locator('.activity button, .activity a').first().evaluate((one) => one.classList.contains('render-here'));
    const was = await button.getAttribute('aria-pressed');
    await button.click();
    await until(`${name}'s chip toggled`, () => button.getAttribute('aria-pressed'), (now) => now !== was, 5000).catch(() => {});
    const pressed = await button.getAttribute('aria-pressed');
    await page.keyboard.press('Escape');
    await page.locator('.activity').waitFor({ state: 'detached' });
    return { pressed };
  };

  // None green: automatic, said once.
  await list();
  const atFirst = await green();
  check('starts_automatic', atFirst.rows.length === 0 && atFirst.chips.length === 0 && (await said()).match(new RegExp(AUTOMATIC, 'g'))?.length === 1, { atFirst, said: await said() });
  check('no_run_on_menu', (await page.locator('.run-on, [aria-label="Render machines"]').count()) === 0);
  const chips = await page.locator('.mtab').evaluateAll((all) => all.map((chip) => chip.dataset.machine));
  check('every_machine_has_a_chip', ['local', IN, OUT].every((name) => chips.includes(name)), chips);

  // Render here on the first stand-in's chip top right, then on local's drawer: each kept by Calcifer, both on at once, each green on its
  // chip, in the list and in its drawer.
  const first = await pressChip(IN);
  const fromChip = await until('the chip\'s choice kept', state, (now) => same(now.machines, [IN])).catch(() => null);
  const chipGreen = await until('its chip green', green, (now) => same(now.chips, [IN])).catch(() => null);
  check('a_chip_toggles_render_here', first.pressed === 'true' && fromChip !== null && chipGreen !== null && seen.flyout_first_control === true, { first, machines: (await state()).machines, green: await green(), first_control: seen.flyout_first_control });

  // Its drawer from the canvas, by its flyout's Details: its face is what it is and what it's doing, Render here first among its buttons,
  // green when on, and the rest behind Details (Paul 11-12).
  await page.goto(at);
  await page.locator(`.mtab[data-machine="${IN}"]`).click();
  await page.locator('.activity a', { hasText: 'Details' }).click();
  await page.locator('.drawer .machine-actions').waitFor();
  const face = await page.evaluate(() => {
    const drawer = document.querySelector('.drawer');
    const first = drawer.querySelector('.machine-actions button');
    const details = [...drawer.querySelectorAll('details.sect')].find((held) => held.querySelector(':scope > summary')?.textContent === 'Details');
    return {
      first: first?.textContent, pressed: first?.getAttribute('aria-pressed'), green: first?.classList.contains('on'),
      doing: drawer.querySelector('.doing-now')?.textContent ?? null, details: Boolean(details), folded: details ? !details.open : null,
      kv_shown: [...drawer.querySelectorAll('.kv')].some((kv) => !kv.closest('details:not([open])')),
    };
  });
  check('drawer_from_the_canvas_leads_with_render_here', face.first === 'Render here' && face.pressed === 'true' && face.green === true, face);
  check('drawer_face_is_slim', face.doing !== null && face.details && face.folded === true && !face.kv_shown, face);
  await shot('drawer');
  const local = await press('local');
  const pinned = await until('both kept', state, (now) => same(now.machines, [IN, 'local'])).catch(() => null);
  check('pressed_shows_pressed', local.pressed === 'true' && local.head === 1, { local });
  check('pinned_in_the_project', pinned !== null, pinned?.machines);
  const both = await until('both green', green, greenAre(['local', IN])).catch(() => null);
  check('green_where_listed', both !== null, await green());
  check('automatic_not_said', !AUTOMATIC.test(await said()), await said());
  await shot('green');
  const status = JSON.parse(cozyIn(folder, ['--json', 'status'], { encoding: 'utf8' }));
  check('cli_reads_the_same', same(status.project?.machines, [IN, 'local']), status.project);
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

  // Pressed again on its chip, the last one comes off: no machine at all, automatic, said again.
  const off = await pressChip(OUT);
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
