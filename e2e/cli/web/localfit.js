// A render on this computer alone, which it would do poorly (Paul, 11-05: "do you want to run locally? rent a GPU"), in a real browser on a real
// Calcifer. A runner has no GPU, so a tool that loads a model on one asks first: under its box, where Queued shows, how it would go here and what a
// rented GPU would do. Render here renders, and is kept by Calcifer: the next Render, and one after a reload, don't ask. Rent a GPU opens Rent with
// the card the Hub places the tool on chosen, and renders nothing. Render all asks once, on its own control. The CLI says the same line once, as
// a warning, and goes on, with no --on as with --on local. With a rental held nothing asks: release-gate/sessions/render-placement.sh.
//   node localfit.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT, $CHROME, $COZY, $SHOT as nodes.js; $ALONE: on a shared computer, so it runs nothing. Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { join } = require('path');
const { boxOf, calcifer, closeBox, importYaml, launch, lastLook, lookAt, noStall, openBox, renderOf, signIn, STEP_MS, until, watchBoxes, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const FIRST_RUN_MS = 300_000;
const COZY = process.env.COZY;
const heavy = join(__dirname, 'heavy.py');
const finish = (code) => {
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(code);
};

(async () => {
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
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  // Only where this computer runs models and has no GPU: anywhere else there's nothing it would do poorly to ask about.
  const gpus = (await api('/v1/machines/local')).body?.status?.gpus;
  seen.gpus = gpus;
  if (!(await api('/v1/status')).body.local_usable || process.env.ALONE || gpus?.length) {
    check('ran', true);
    await browser.close();
    return finish(0);
  }

  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-localfit-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  const versions = async (name) => (await state()).assets.find((held) => held.name === name)?.versions ?? 0;
  // A render's runs of `fn`, not its package's install.
  const runsOf = async (fn) => ((await api(`/v1/runs?project=${id}&limit=100`)).body?.runs ?? []).filter((run) => new RegExp(`^\\S+/${fn}(@\\S+)?$`).test(run.target ?? ''));
  const fit = async (names) => (await api(`/v1/projects/${id}/fit`, { method: 'POST', body: JSON.stringify({ names }) })).body;
  const box = (name) => boxOf(page, name);
  const asking = (name) => box(name).locator('.status.asking');
  // Render is with its words: on a take, its + goes to them. Just after a reload the box can still turn to its newest take as its versions come,
  // taking Render from under the press: a person presses again.
  const render = async (name) => {
    await openBox(page, name);
    await until(`${name}'s Render pressed`, async () => (await renderOf(page, name)).click({ timeout: 5000 }).then(() => true, () => false), Boolean);
  };
  // Asked, it would have said so by the time a render's Queued gives way to its run; a weightless one is quick, so this watches until it's made.
  const neverAsks = async (name, made) => {
    let asked = false;
    await until(`${name}'s version ${made}`, async () => {
      asked ||= (await asking(name).count()) > 0;
      return asked || (await versions(name)) >= made;
    }, Boolean, FIRST_RUN_MS);
    return !asked;
  };

  // A still whose tool composes a GPU function that loads a model, not yet rendered here. Named before it's run, it's read by PyPI's Runtime, which
  // knows no tool; the install a run makes on this machine's Runtime takes its place, as tools.js primes its fixture.
  execFileSync(COZY, ['--json', 'run', `${heavy}/heavy`, 'instructions=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'pipe'], timeout: FIRST_RUN_MS });
  const primed = (await runsOf('heavy')).length;
  importYaml(folder, `version: 4\nassets:\n  heavy:\n    tool: ${heavy}/heavy\n    instructions: a lantern in fog\n`);
  await box('heavy').waitFor();

  // 1. The first Render asks, on the box, and nothing renders.
  await render('heavy');
  await asking('heavy').waitFor({ timeout: FIRST_RUN_MS });
  const said = await asking('heavy').innerText();
  check('asks_on_the_box', /no GPU/.test(said) && /Render here/.test(said) && /Rent a GPU/.test(said), said);
  seen.line = said;
  // Render keeps the box open, on its words: what it asks is under them, where Queued would be, and nothing covers the canvas. Left, it's under
  // the box at rest.
  const overCanvas = () => page.locator('dialog[open], [role=dialog]').count();
  check('asks_in_the_box', (await page.locator('.card.here[data-asset="heavy"] .here-under .status.asking').count()) === 1 && (await overCanvas()) === 0, await box('heavy').innerText());
  await closeBox(page);
  check('asks_under_the_box', (await box('heavy').locator('.rest-asking .status.asking').count()) === 1 && (await overCanvas()) === 0);
  await page.waitForTimeout(1000);
  check('asking_renders_nothing', (await runsOf('heavy')).length === primed && (await versions('heavy')) === 0, await runsOf('heavy'));
  await shot('asks');

  // 2. Render here renders, and Calcifer keeps it.
  await asking('heavy').locator('button:has-text("Render here")').click();
  check('render_here_renders', Boolean(await until('heavy rendered here', () => versions('heavy'), (n) => n >= 1, FIRST_RUN_MS)));
  const kept = await fit(['heavy']);
  check('kept_by_calcifer', kept?.tools?.length === 1 && kept.tools[0].here === true, kept);
  check('rendered_here', (await runsOf('heavy')).every((run) => run.machine === 'local'), (await runsOf('heavy')).map((run) => run.machine));

  // 3. The next Render doesn't ask.
  await render('heavy');
  check('second_render_does_not_ask', await neverAsks('heavy', 2));

  // 4. Nor after a reload.
  await page.reload();
  await box('heavy').waitFor();
  await render('heavy');
  check('reload_does_not_ask', await neverAsks('heavy', 3));

  // 6. Qwen Image from the Hub, as 0.22's tool list drops it. Rent a GPU opens Rent with the card the Hub places it on, and renders nothing.
  // Only if it asks: a runner can't hold its weights.
  importYaml(folder, `${yamlOf(folder).trimEnd()}\n  kite:\n    tool: qwen-image-2/image\n    instructions: a red kite over dunes\n`);
  // Drawn in on heavy, the canvas draws only what's near it: back out, and all of it in view.
  await closeBox(page);
  await page.keyboard.press('f');
  await box('kite').waitFor();
  const kiteFit = await fit(['kite']);
  const placed = kiteFit?.rent;
  seen.placed = placed;
  if (!kiteFit?.tools?.length) {
    check('kite_asks', false, kiteFit);
    await browser.close();
    return finish(1);
  }
  await render('kite');
  await asking('kite').waitFor({ timeout: STEP_MS });
  const kiteSaid = await asking('kite').innerText();
  check('kite_asks', /no GPU/.test(kiteSaid) && (!placed || kiteSaid.includes(placed.line)), kiteSaid);
  await shot('kite-asks');
  await asking('kite').locator('button:has-text("Rent a GPU")').click();
  await until('Machines open', () => Promise.resolve(page.url()), (url) => url.includes('/machines'));
  check('rent_opens', true);
  if (placed) {
    await page.locator('.sku.chosen').waitFor({ timeout: STEP_MS });
    const chosen = await page.locator('.sku.chosen b').innerText();
    check('rent_has_it_chosen', chosen === placed.sku && page.url().includes(`rent=${placed.sku}`), { chosen, placed: placed.sku, url: page.url() });
    await shot('rent');
  } else check('rent_has_it_chosen', false, 'the Hub placed no card for qwen-image-2/image');
  await page.waitForTimeout(1500);
  check('rent_renders_nothing', (await runsOf('image')).length === 0 && (await versions('kite')) === 0, await runsOf('image'));
  await page.goBack();
  await closeBox(page);

  // Render all: once, on its own control, for what's still unsaid (Qwen Image only), and Rent a GPU there renders nothing either.
  const before = await versions('heavy');
  await page.locator('.render-all button.primary').click();
  const all = page.locator('.render-all .status.asking');
  await all.waitFor({ timeout: STEP_MS });
  const allSaid = await all.innerText();
  check('render_all_asks_once', (await page.locator('.status.asking').count()) === 1 && /Qwen|qwen|kite/i.test(allSaid) && !/heavy/i.test(allSaid), allSaid);
  await shot('render-all-asks');
  await all.locator('button:has-text("Rent a GPU")').click();
  await page.waitForTimeout(1500);
  check('render_all_rent_renders_nothing', (await versions('heavy')) === before && (await runsOf('image')).length === 0);
  await page.goBack();

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
