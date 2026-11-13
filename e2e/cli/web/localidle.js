// This computer's machine stops while a project is open, as its 15 idle minutes stop it, and there is no other machine (cl-894): Render
// all, with no machine chosen, starts it and renders there. Before, the page counted only a `ready` local, and with none asked to rent one.
//   node localidle.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT, $CHROME, $COZY, $SHOT as
// nodes.js. Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { readFileSync } = require('fs');
const { dirname, join } = require('path');
const { holderToken, projectFolder, calcifer, lastLook, launch, lookAt, noStall, projectAction, projectFromPage, signIn, STEP_MS, until, watchBoxes } = require('./page');

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
// TensorD's lock record, beside Calcifer's in the home.
const HOLDER = join(dirname(process.env.HOLDER), '..', 'machine', 'var', 'lib', 'cozy', 'machine', 'agent.lock.holder');

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
  const token = holderToken();
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json();
  const machines = async () => (await api('/v1/machines')).machines ?? [];
  const local = async () => (await machines()).find((machine) => machine.name === 'local');
  check('no_other_machine', (await machines()).every((machine) => machine.name === 'local' || machine.state === 'ended'), (await machines()).map((machine) => machine.name));

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-localidle-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  await projectAction(folder, 'add', `${TOOL}/still`, 'idle_box', 'at=200,400,260', 'instructions=An idle still');
  await page.locator('.render-all button.primary').waitFor();

  // Stopped as its idle clock stops it: a clean exit, while the page is open.
  const pid = JSON.parse(readFileSync(HOLDER, 'utf8')).pid;
  process.kill(pid, 'SIGTERM');
  const stopped = await until('local stopped', local, (machine) => machine?.state === 'stopped').catch(() => null);
  check('stopped_before', stopped !== null, (await local())?.state);
  // The page hears it before the press (cl-894's bug was the page counting only a ready local): its chip says it's stopped.
  const chip = page.locator('.mtabs [data-machine="local"]');
  const heard = await until('the page shows local stopped', () => chip.getAttribute('title').catch(() => ''), (title) => /\bstopped\b/.test(title ?? '')).then(() => true, () => false);
  check('the_page_shows_it_stopped', heard, await chip.getAttribute('title').catch(() => null));
  await shot('stopped');

  await page.locator('.render-all button.primary').click();
  const held = (now) => now.assets?.find((asset) => asset.name === 'idle_box');
  const made = await until('idle_box made or failed', state, (now) => !now.busy && Boolean(held(now)?.versions >= 1 || held(now)?.failure), RENDER_MS).catch(() => null);
  check('rendered', made !== null && held(made)?.versions >= 1, made ? held(made)?.failure?.message ?? held(made)?.status : 'nothing in time');
  const runs = ((await api(`/v1/projects/${id}/runs?limit=100`)).runs ?? []).filter((run) => /\/still(@\S+)?$/.test(run.target ?? ''));
  seen.placed = runs.map((run) => `${run.number} ${run.machine} ${run.status}`);
  check('ran_here', runs.length >= 1 && runs.every((run) => run.machine === 'local' && run.status === 'completed'), seen.placed);
  check('started', (await local())?.state === 'ready', (await local())?.state);
  await shot('rendered');

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
