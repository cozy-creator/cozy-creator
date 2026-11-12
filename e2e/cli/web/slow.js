// A run is never silently slow (cl-913), on the page: local, whose machine reads a hot host's recorded GPU readings (slow.sh), is red
// on its chip top right, and its flyout and drawer say why; a box whose run there loads its GPU 30 s says on the box that a GPU throttled
// for heat and that the run was far slower than its tool took before.
//   node slow.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT, $CHROME, $COZY, $SHOT as
// nodes.js. Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { join } = require('path');
const { calcifer, launch, lastLook, lookAt, noStall, projectAction, projectFromPage, signIn, STEP_MS, until, watchBoxes } = require('./page');

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
const RENDER_MS = 300_000;
const HEAT = /GPU 0 ran throttled for heat 98% of its last run \(89 °C, \d+ of 1755 MHz\)/;

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
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}` } })).json();

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-slow-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const at = page.url();
  const id = (await projectFromPage(page)).id;
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  await projectAction(folder, 'add', `${TOOL}/slow`, 'hot_box', 'at=300,300,260', 'seconds:=30', 'instructions=A still on a hot host');

  // Its chip top right: red, and its flyout says why.
  const chip = page.locator('.mtab[data-machine="local"]');
  await chip.waitFor();
  const red = await until('local red', async () => ({ slow: await chip.getAttribute('data-slow'), dots: await chip.locator('.dot.slow').count() }), (now) => now.slow !== null && now.dots === 1, 15_000).catch(() => null);
  check('chip_red', red !== null, { slow: await chip.getAttribute('data-slow'), html: await chip.innerHTML() });
  await chip.click();
  const flyout = await page.locator('.activity').innerText();
  check('flyout_says_why', HEAT.test(await page.locator('.activity p.warn.slow').first().innerText().catch(() => '')), flyout);
  check('flyout_line_red', await page.locator('.activity p.warn.slow').first().evaluate((line) => getComputedStyle(line).color).catch(() => '') === 'rgb(217, 48, 37)');
  await shot('chip');
  await page.keyboard.press('Escape');

  // Its drawer says it too.
  await page.goto(`${at}/machines/local`);
  await page.locator('.drawer p.warn.slow').first().waitFor();
  check('drawer_says_why', HEAT.test(await page.locator('.drawer p.warn.slow').first().innerText()), await page.locator('.drawer').innerText());
  await shot('drawer');

  // The box's run there: what made it slow, on the box.
  await page.goto(at);
  await page.locator('.render-all button.primary').click();
  const made = await until('the box made', state, (now) => !now.busy && now.assets.some((held) => held.name === 'hot_box' && (held.versions >= 1 || held.failure)), RENDER_MS).catch(() => null);
  const held = made?.assets.find((one) => one.name === 'hot_box');
  check('box_made', held?.versions >= 1, held ?? (await state()).assets);
  const said = (await state()).warnings?.filter((warning) => warning.name === 'hot_box').map((warning) => warning.message) ?? [];
  seen.said = said;
  check('box_record_says_heat', said.some((message) => /GPU 0 ran throttled for heat 98% of this run/.test(message)), said);
  check('box_record_says_slow', said.some((message) => /took .*×.*Likely: .*throttled for heat/.test(message) || /has run .*×.*Likely: .*throttled for heat/.test(message)), said);
  // Open (by its address: a click only selects it), its warnings are its heading's !, and pressed, its lines.
  await page.goto(`${at}/assets/hot_box`);
  await page.locator('[data-focused-asset="hot_box"], .card.here[data-asset="hot_box"]').first().waitFor();
  const warned = page.locator('[data-focused-asset="hot_box"] .focus-heading .iconb.warned, .card.here[data-asset="hot_box"] .head .iconb.warned').first();
  const title = await until('the box warned', () => warned.getAttribute('title').catch(() => null), (now) => Boolean(now) && /throttled for heat/.test(now) && /×/.test(now), 30_000).catch(() => null);
  check('box_shows_it', title !== null, await warned.getAttribute('title').catch(() => null));
  await warned.click().catch(() => {});
  const lines = await page.locator('[data-focused-asset="hot_box"] .warned-lines, .card.here[data-asset="hot_box"] .warned-lines').first().innerText().catch(() => '');
  check('box_lines_say_it', /throttled for heat/.test(lines) && /×/.test(lines), lines);
  await shot('box');

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
