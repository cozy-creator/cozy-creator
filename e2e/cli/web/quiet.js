// A run whose fallbacks the Runtime caught and recovered from completes quietly on its box (cl-921, cozy-runtime #2145): BiRefNet
// on this computer's CPU computes its fp16 weights in float32, which its log once said as a warning. While it renders and once its
// take lands, its box shows no warning (no `.face-warning` line, no Warnings icon), no failure, and no note of an error.
//   node quiet.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots; $ALONE: on a shared computer,
// so it runs nothing. Prints what it saw as JSON and exits 1 if anything failed. Its box on the canvas is kept as a PNG each time its
// face's phase or state line changes, `phases/<n>-<phase>.png` beside $SHOT, and its Focus view once (Paul, 11-12: the box at each
// phase, cl-921).
const { chromium } = require(process.env.PLAYWRIGHT);
const { mkdirSync, writeFileSync } = require('fs');
const { dirname, join } = require('path');
const { tmpdir } = require('os');
const { calcifer, closeBox, lastLook, lookAt, openBox, png, projectAction, projectFromPage, renderOf, signIn, STEP_MS, until } = require('./page');

const failures = [];
const seen = {};
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}

const RUN_MS = 600_000;
const NAME = 'cutout';

/** The box named `name` as the page draws it on this screen: what it warns of, whether it failed, its take, and the notes up. */
const LOOK = (name) => {
  const card = document.querySelector(`.card.here[data-asset="${CSS.escape(name)}"]`)
    ?? [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector('.nm')?.textContent === name)?.querySelector('.card');
  const node = card?.closest('.react-flow__node') ?? card;
  return {
    drawn: Boolean(card),
    face: node?.querySelector('.run-face')?.getAttribute('data-phase') ?? null,
    says: node?.querySelector('.run-face .face-state b')?.textContent ?? null,
    warnings: [...(node?.querySelectorAll('.face-warning') ?? [])].map((line) => line.textContent),
    warned: Boolean(node?.querySelector('.warned')),
    failed: Boolean(node?.querySelector('.run-face.failed, [data-phase="failed"], .glimpse.failed')),
    take: Boolean(node?.querySelector('.media-pixels img')),
    making: Boolean(node?.querySelector('.media-making')),
    notes: [...document.querySelectorAll('.note .line')].map((line) => line.textContent),
  };
};

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  seen.page_errors = errors;
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const response = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: response.status, body: await response.json().catch(() => null) };
  };
  const local = Boolean((await api('/v1/status')).body?.local_usable) && !process.env.ALONE;
  seen.ran = local;
  if (!local) {
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    return;
  }

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-quiet-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const folder = (await api(`/v1/projects/${id}`)).body.path;
  seen.project = { id, folder };
  const project = (...words) => projectAction(folder, ...words);
  const look = () => page.evaluate(LOOK, NAME);
  const versions = async () => ((await api(`/v1/projects/${id}`)).body?.assets?.find((row) => row.name === NAME)?.versions ?? []).length;

  // A picture, and BiRefNet on it: its picture slot starts from the box's own (tools §9).
  during = 'a picture with BiRefNet on it';
  const picture = join(tmpdir(), `web-quiet-${Date.now()}.png`);
  writeFileSync(picture, png(90));
  await project('import', picture, NAME);
  await until('its box', look, (now) => now.drawn);
  const tools = (await api(`/v1/tools?project=${id}&q=birefnet`)).body?.tools ?? [];
  const cutout = tools.find((tool) => tool.package === 'birefnet' && tool.makes === 'image');
  seen.tool = cutout?.id ?? tools.map((tool) => tool.id);
  if (!cutout) throw new Error('the Hub lists no BiRefNet tool');
  const put = await api(`/v1/projects/${id}/assets/${NAME}`, { method: 'PATCH', body: JSON.stringify({ tool: cutout.id }) });
  if (put.status >= 300) throw new Error(`BiRefNet on ${NAME}: ${put.status} ${JSON.stringify(put.body)}`);

  // Rendered on this computer, looked at as it goes: every warning, failure or error note it shows on the way is kept.
  // Rendered from its Render, as a person does (an image tool renders from its Edit), its Focus view kept once its face shows.
  during = 'its render';
  const had = await versions();
  const phases = process.env.SHOT ? join(dirname(process.env.SHOT), 'phases') : null;
  if (phases) mkdirSync(phases, { recursive: true });
  await openBox(page, NAME);
  await (await renderOf(page, NAME)).click();
  const focus = page.locator(`.card.here[data-asset="${NAME}"]`).first();
  const faced = await focus.locator('.run-face').first().waitFor({ timeout: 60_000 }).then(() => true, () => false);
  if (phases) await focus.screenshot({ path: join(phases, '00-focus.png') }).catch(() => {});
  seen.focus_face = faced;
  await closeBox(page);
  const box = page.locator('.react-flow__node').filter({ has: page.locator(`.card[data-asset="${NAME}"]`) }).first();
  let last = '';
  let kept = 0;
  const said = { warnings: new Set(), notes: new Set(), faces: new Set(), warned: false, failed: false };
  const landed = await until('its new take, quietly', async () => {
    const now = await look();
    const state = `${now.face}|${now.says}`;
    if (now.face && state !== last && phases) {
      kept += 1;
      await box.screenshot({ path: join(phases, `${String(kept).padStart(2, '0')}-${now.face}.png`) }).catch(() => {});
    }
    last = state;
    now.warnings.forEach((line) => said.warnings.add(line));
    now.notes.forEach((line) => said.notes.add(line));
    if (now.face) said.faces.add(`${now.face}: ${now.says}`);
    said.warned ||= now.warned;
    said.failed ||= now.failed;
    return { ...now, versions: await versions() };
  }, (now) => (now.versions > had && now.take && !now.making && !now.face) || now.failed, RUN_MS);
  await page.waitForTimeout(1500);
  const after = await look();
  await shot('landed');
  seen.render = { landed, after, faces: [...said.faces], warnings: [...said.warnings], notes: [...said.notes], screenshots: kept };
  check('its_take_lands', landed.versions > had && landed.take, landed);
  check('its_box_said_how_it_went', said.faces.size > 0, [...said.faces]);
  check('no_warning_line_while_it_renders', said.warnings.size === 0, [...said.warnings]);
  check('no_warnings_icon', !said.warned && !after.warned, after);
  check('no_failure', !said.failed && !after.failed, seen.render);
  check('no_error_note', ![...said.notes, ...after.notes].some((line) => /error|failed|warn/i.test(line)), [...said.notes, ...after.notes]);

  // What its run's record says: no warnings, and no `warn` adjustment.
  const runs = (await api('/v1/runs?limit=20')).body?.runs ?? [];
  const run = runs.find((one) => one.target?.includes('birefnet') && one.status === 'completed');
  seen.run = run && { number: run.number, warnings: run.warnings ?? [], adjustments: run.adjustments ?? [] };
  check('its_run_warns_of_nothing', Boolean(run) && !(run.warnings ?? []).length && !(run.adjustments ?? []).some((one) => one.kind === 'warn'), seen.run);
  check('no_page_errors', errors.length === 0, errors);

  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen, during }, null, 2));
  process.exit(2);
}
