// A render's progress on its own box only, and a box waiting for another's take preparing at once (cl-905). A project pinned to local
// and a stand-in, so where the waiting box will run is a guess: `pic` (a slow still) and `clip` (a video tool in an environment of its
// own) whose words mention <pic>. Render all: while pic is made, only pic's box shows its run; clip's says it's waiting for <pic>,
// with none of pic's stage or numbers, and shows its own preparation, its tool's environment on the machine it will run on (its face's
// machine line), asked within seconds of Render, ready before its turn.
//   node prepares.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER; $STANDS: a stand-in machine added
// there (machineset.sh). $PLAYWRIGHT, $CHROME, $COZY, $SHOT as nodes.js. Prints what it saw as JSON, exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { join } = require('path');
const { boxOf, calcifer, cozyIn, lastLook, launch, lookAt, noStall, projectAction, projectFromPage, signIn, STEP_MS, until, watchBoxes } = require('./page');

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
const CLIP = join(__dirname, '..', 'project', 'clip.py');
const [STAND] = (process.env.STANDS ?? '').split(' ');
const PIC_S = 60;
const RENDER_MS = 420_000;
// What pic's run says of its work: its stage and its count.
const PICS_WORK = /drawing|\d+ \/ \d+|step/i;

(async () => {
  if (!STAND) {
    check('stand_in', false, process.env.STANDS ?? 'none');
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
  const { token } = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session')));
  const api = async (path) => (await fetch(base + path, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json();

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-prepares-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const at = page.url();
  const id = (await projectFromPage(page)).id;
  const state = () => api(`/v1/projects/${id}`);
  const folder = (await state()).path;
  seen.project = { id, folder };
  cozyIn(folder, ['machine', 'use', `local,${STAND}`]);
  await projectAction(folder, 'add', `${TOOL}/slow`, 'pic', 'at=200,400,260', 'instructions=a picture to wait for', `seconds=${PIC_S}`);
  await projectAction(folder, 'add', `${CLIP}/clip`, 'clip', 'at=600,400,260', 'instructions=<pic> held still');
  await page.goto(at);
  await boxOf(page, 'pic').waitFor();
  await boxOf(page, 'clip').waitFor();

  // Each box's face as drawn, its two lines: what it's doing, and the machine.
  const face = (name) => boxOf(page, name).locator('.run-face').first().evaluate((el) => ({
    phase: el.dataset.phase,
    says: el.querySelector('.face-state b')?.textContent ?? '',
    machine: el.querySelector('.face-machine')?.textContent ?? '',
  }), null, { timeout: 500 }).catch(() => null);
  const making = (now, name) => now.assets?.find((held) => held.name === name)?.making ?? [];

  const began = Date.now();
  await page.locator('.render-all button.primary').click();
  const samples = [];
  for (;;) {
    const now = await state();
    const ahead = (now.board?.ahead ?? []).find((row) => (row.names ?? []).includes('clip'));
    const sample = {
      s: Math.round((Date.now() - began) / 100) / 10,
      pic_making: making(now, 'pic'),
      clip_making: making(now, 'clip'),
      pic: await face('pic'),
      clip: await face('clip'),
      ahead: ahead && { run: ahead.run, machine: ahead.machine, stage: ahead.stage ?? null, done: ahead.done, total: ahead.total, ready: Boolean(ahead.ready), held_back: ahead.held_back ?? null, waiting: ahead.waiting ?? null },
    };
    samples.push(sample);
    if (sample.clip_making.length || Date.now() - began > RENDER_MS) break;
    if (samples.length === 6) await shot('waiting');
    await page.waitForTimeout(500);
  }
  seen.samples = samples.filter((sample, at) => at === 0 || JSON.stringify({ ...sample, s: 0 }) !== JSON.stringify({ ...samples[at - 1], s: 0 }));

  // While pic is made: its box shows its run, and clip's shows none of it.
  const whilePic = samples.filter((sample) => sample.pic_making.length && !sample.clip_making.length);
  check('pic_rendered_first', whilePic.length > 0, samples.slice(0, 3));
  check('pic_shows_its_own', whilePic.some((sample) => PICS_WORK.test(sample.pic?.says ?? '')), whilePic.map((sample) => sample.pic));
  const borrowed = whilePic.filter((sample) => sample.clip && (PICS_WORK.test(`${sample.clip.says} ${sample.clip.machine}`) || !/^(Waiting for|Queued)/.test(sample.clip.says)));
  check('clip_shows_none_of_pics', borrowed.length === 0, borrowed.slice(0, 3));
  check('clip_says_it_waits', whilePic.some((sample) => /^Waiting for <pic>/.test(sample.clip?.says ?? '')), whilePic.slice(0, 3).map((sample) => sample.clip));

  // Its own preparation, asked at once, on a machine of its set, said on its box, ready before its turn.
  const asked = samples.find((sample) => sample.ahead);
  const ready = samples.find((sample) => sample.ahead?.ready);
  const picFirst = samples.findIndex((sample) => sample.pic_making.length);
  const picDone = picFirst < 0 ? undefined : samples.slice(picFirst).find((sample) => !sample.pic_making.length);
  seen.timeline = { ahead_asked_s: asked?.s ?? null, ahead_ready_s: ready?.s ?? null, pic_done_s: picDone?.s ?? null, clip_run_s: samples.at(-1).clip_making.length ? samples.at(-1).s : null, on: asked?.ahead.machine ?? null };
  // At once: while pic is made, not after; its first quote waits for nothing but its own package's description.
  const drawing = samples.find((sample) => PICS_WORK.test(sample.pic?.says ?? ''));
  seen.timeline.pic_drawing_s = drawing?.s ?? null;
  check('prepares_at_once', asked !== undefined && asked.pic_making.length > 0 && (drawing === undefined || asked.s <= drawing.s + 10), seen.timeline);
  check('prepares_in_its_set', ['local', STAND].includes(asked?.ahead.machine), asked?.ahead);
  check('shows_its_preparation', whilePic.some((sample) => / - (Preparing: |Ready$)/.test(sample.clip?.says ?? '') && ['local', STAND].includes(sample.clip?.machine)), whilePic.map((sample) => sample.clip));
  check('ready_before_its_turn', ready !== undefined && (picDone === undefined || ready.s <= picDone.s + 1), seen.timeline);
  check('nothing_held_back', !samples.some((sample) => sample.ahead?.held_back), samples.find((sample) => sample.ahead?.held_back)?.ahead);
  await shot('clip-going');

  const settled = (held) => held && (held.versions >= 1 || held.failure);
  const made = await until('both made or failed', state, (now) => !now.busy && ['pic', 'clip'].every((name) => settled(now.assets.find((held) => held.name === name))), RENDER_MS).catch(() => null);
  seen.failed = (made?.assets ?? []).filter((held) => ['pic', 'clip'].includes(held.name) && held.failure).map((held) => `${held.name}: ${held.failure.message}`);
  // The video's own run, prepared ahead: from sent to started, it waits for nothing of its own.
  const clipRun = samples.at(-1).clip_making[0];
  const run = clipRun === undefined ? null : await api(`/v1/projects/${id}/runs/${clipRun}`);
  if (run?.started_at_ms) seen.timeline.clip_sent_to_started_s = Math.round((run.started_at_ms - run.created_at_ms) / 100) / 10;
  check('rendered', made !== null && seen.failed.length === 0, seen.failed);

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
