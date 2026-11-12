// A shot rendered segment by segment plays all it has made so far while the next renders (cl-907), with project/segments.py, H3's
// stand-in: three segments of 10 s, each after the first taking 30 s. Segment 1 lands and its box shows it, nothing over it but how far
// the render has got: opened, it waits with its controls, seeks, and plays with its sound. Segments 1-2 land as one video where it was
// paused. At rest it plays silent. Played as the take lands, the take takes its place and plays on from there.
//   node segments.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER. $PLAYWRIGHT, $CHROME, $COZY, $SHOT,
//   $ALONE as nodes.js. Where this computer runs no models it renders nothing. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { boxOf, calcifer, closeBox, importYaml, lastLook, launch, lookAt, noStall, openBox, projectFromPage, renderOf, signIn, STEP_MS, until, watchBoxes } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
const RUN_MS = 300_000;
const FIXTURE = join(__dirname, '..', 'project', 'segments.py');
const near = (a, b, by = 0.6) => Math.abs(a - b) <= by;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  await watchBoxes(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  await page.addInitScript(() => {
    window.violations = [];
    document.addEventListener('securitypolicyviolation', (e) => window.violations.push(`${e.violatedDirective}: ${e.blockedURI}`));
  });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null), answer };
  };
  const local = (await api('/v1/status')).body.local_usable && !process.env.ALONE;
  seen.ran = local;
  if (!local) {
    console.log(JSON.stringify({ failures, seen }, null, 2));
    await browser.close();
    return;
  }

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-segments-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const state = async () => (await api(`/v1/projects/${id}`)).body;
  const folder = (await state()).path;
  seen.project = { id, folder };
  const fixture = join(folder, 'segments.py');
  writeFileSync(fixture, `${readFileSync(FIXTURE, 'utf8')}\n# ${id}\n`);
  // A box names it, so the project lists it; its first run installs it, by the CLI as a person's would (as tools.js).
  // The new project's whole file, as web.sh's web_held writes one: it has no other box.
  importYaml(folder, `version: 4\nassets:\n  shot:\n    tool: ${fixture}/segments\n    summary: Along the coast, segment after segment.\n    segments: 3\n    seconds: 10\n    wait: 30\n`);
  const box = boxOf(page, 'shot');
  await box.waitFor();
  execFileSync(process.env.COZY, ['--json', 'run', `${fixture}/segments`, 'segments=1', 'seconds=1', 'wait=0', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: RUN_MS });
  await until('the fixture listed', async () => (await api(`/v1/tools?project=${id}`)).body.tools.find((held) => held.local && held.id === `${fixture}/segments`), Boolean, RUN_MS);

  await openBox(page, 'shot');
  await (await renderOf(page, 'shot')).click();
  const making = await until('its run', async () => (await state()).assets.find((held) => held.name === 'shot')?.making?.[0], Boolean, RUN_MS);
  seen.run = making;
  const video = box.locator('.glimpse video');
  const facts = () => video.evaluate((el) => ({
    label: el.closest('.glimpse').dataset.label, rev: Number(el.closest('.glimpse').dataset.rev), duration: el.duration, at: el.currentTime, paused: el.paused,
    muted: el.muted, controls: el.controls, ready: el.readyState, seekable: el.seekable.length ? el.seekable.end(0) : 0, audio: el.webkitAudioDecodedByteCount ?? null,
  })).catch(() => null);

  // Segment 1 lands while segment 2 renders: opened, it waits with its controls and its sound on, seeks, and plays.
  const first = await until('segment 1 on the box', facts, (now) => now?.label === 'Segment 1 of 3' && now.ready >= 1, RUN_MS);
  const going = async () => (await api(`/v1/runs/${making}`)).body;
  const during2 = await until('segment 2 rendering', going, (run) => run.status !== 'in_progress' || /Segment 2 of 3/.test(run.progress?.stage ?? ''), 15_000);
  seen.segment_1 = { ...first, stage: during2.progress?.stage };
  check('segment_1_lands_while_segment_2_renders', during2.status === 'in_progress' && /Segment 2 of 3/.test(during2.progress?.stage ?? ''), during2.progress);
  check('segment_1_is_its_ten_seconds', near(first.duration, 10) && near(first.seekable, 10), first);
  check('opened_it_waits_with_its_controls_and_sound', first.controls && !first.muted && first.paused, first);
  check('nothing_hides_it', (await box.locator('.media-making').count()) === 0 && await box.locator('.glimpse video').evaluate((el) => {
    const { x, y, width, height } = el.getBoundingClientRect();
    return document.elementFromPoint(x + width / 2, y + height / 2) === el;
  }), await box.evaluate((el) => [...el.querySelectorAll('.media > *')].map((child) => child.className)));
  const bytes = async (rev) => Buffer.from(await (await api(`/v1/runs/${making}/outputs/video?rev=${rev}`)).answer.arrayBuffer());
  const file1 = await bytes(first.rev);
  check('segment_1_has_picture_and_sound', file1.includes('avc1') && file1.includes('mp4a'), file1.length);
  await video.evaluate((el) => { el.currentTime = 4; });
  await until('seeked to 0:04', facts, (now) => near(now.at, 4, 0.3));
  await video.evaluate((el) => el.play());
  const played = await until('it plays', facts, (now) => now.at > 5.2 && !now.paused, 15_000);
  seen.played = played;
  check('it_plays_with_its_sound', played.audio === null || played.audio > 0, played);
  await shot('segment-1-open');
  await video.evaluate((el) => el.pause());
  const paused = (await facts()).at;

  // Segments 1-2 land as one video, where it was.
  const second = await until('segments 1-2 on the box', facts, (now) => now?.label === 'Segments 1-2 of 3' && now.ready >= 1 && near(now.duration, 20), 90_000);
  seen.segments_1_2 = second;
  check('segments_1_2_are_one_twenty_second_video', near(second.duration, 20) && near(second.seekable, 20), second);
  check('it_stays_where_it_was', near(second.at, paused, 0.3) && second.paused, { paused, second });
  const file2 = await bytes(second.rev);
  check('segments_1_2_have_picture_and_sound', file2.includes('avc1') && file2.includes('mp4a') && file2.length > file1.length, { first: file1.length, second: file2.length });

  // At rest it plays silent, how far the render has got along with it.
  await closeBox(page);
  const rest = await until('it plays at rest', async () => {
    const one = await facts();
    await page.waitForTimeout(400);
    const two = await facts();
    return one && two && { ...two, moved: two.at !== one.at };
  }, (now) => now?.moved && now.muted && !now.controls, 15_000).catch((error) => error.message);
  check('at_rest_it_plays_silent', rest?.moved === true, rest);
  check('at_rest_its_progress_shows_with_it', (await box.locator('.prog, .run-face').count()) > 0 && (await box.locator('.media-making').count()) === 0);
  await shot('segments-1-2-rest');

  // Played as segment 3 ends, the take lands in its place and plays on from where it was.
  await openBox(page, 'shot');
  await until('segment 3 nearly done', going, (run) => run.status !== 'in_progress' || (/Segment 3 of 3/.test(run.progress?.stage ?? '') && run.progress.completed >= 6), 90_000);
  const where = await box.locator('.media').boundingBox();
  await video.evaluate((el) => { el.currentTime = 0; return el.play(); });
  const take = await until('the take', state, (now) => now.assets.find((held) => held.name === 'shot')?.prefixes?.length === 1, RUN_MS);
  seen.take = take.assets.find((held) => held.name === 'shot').prefixes[0].main;
  const live = box.locator('video.live');
  // First seen playing: from where the glimpse was, some seconds in, not from its start.
  const after = await until('the take playing on', () => live.evaluate((el) => ({ at: el.currentTime, paused: el.paused, duration: el.duration })).catch(() => null), (now) => now && !now.paused && now.at > 0, 30_000).catch((error) => error.message);
  seen.take_playing = after;
  check('the_take_plays_on_from_where_it_was', typeof after === 'object' && after.at >= 4 && near(after.duration, 30, 1), after);
  check('no_glimpse_once_it_lands', (await box.locator('.glimpse').count()) === 0);
  const now = await box.locator('.media').boundingBox();
  check('the_take_is_in_its_place', ['x', 'y', 'width', 'height'].every((key) => Math.abs(now[key] - where[key]) <= 2), { where, now });
  await shot('take');
  await closeBox(page);

  check('csp_holds', (await page.evaluate(() => window.violations)).length === 0, await page.evaluate(() => window.violations));
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
