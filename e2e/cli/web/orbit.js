// Files carry metadata, and an orbit turns (xs-180, Paul 11-11), in a real browser on a real Calcifer. A function's take whose file it
// declared `orbit` (project/orbit.py, every frame a keyframe) lands with that map in project.db and on the page; its open box has no play
// button, and a drag across it turns it, wrapping past 0°, the frame `floor(angle / 360 × frames) mod frames`. Its toggle turns orbit viewing
// off and back by Undo, and on for a dropped video that never had a version.
//   node orbit.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER, on a computer whose own machine runs the tool.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, calcifer, signIn, MP4, STEP_MS, boxOf, lastLook, launch, lookAt, noStall, openBox, closeBox, settled, until, addBoxes, upload, rowsOf } = require('./page');

const fixture = join(__dirname, '..', 'project', 'orbit.py');
const FRAMES = 48;
const ORBIT = { degrees: 360, direction: 'cw', loops: true };
const failures = [];
const seen = {};
noStall(seen, died);
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
async function died(why) {
  failures.push(`${during}: ${why}`);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(1);
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json', ...(init.headers ?? {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-orbit-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state())?.assets?.find((row) => row.name === name);
  const folder = (await state()).path;
  seen.project = { id, folder };
  const shown = (row) => row?.prefixes?.find((take) => take.number === row.version)?.main ?? row?.prefixes?.at(-1)?.main;
  /** What project.db holds of `name`'s takes: each record's metadata. */
  const stored = () => rowsOf(folder, 'SELECT record FROM versions WHERE discarded = 0').map(([record]) => JSON.parse(record)).filter((record) => record.metadata);

  // A box of the tool, and a dropped video beside it.
  const clips = mkdtempSync(join(tmpdir(), 'web-orbit-'));
  writeFileSync(join(clips, 'plain.mp4'), MP4);
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(clips, 'plain.mp4'), id), name: 'plain', exact: true, at: { x: 700, y: 100 } }) });
  addBoxes(folder, [{ name: 'spin', tool: `${fixture}/turn`, kind: 'video', fields: { frames: FRAMES } }]);
  await until('spin placed', async () => (await api(`${at}/canvas`)).body?.cards?.spin, Boolean);
  await api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names: ['spin'], on: ['local'] }), headers: { 'Idempotency-Key': `orbit-${Date.now()}` } });
  // Its file here and measured, as a take landed is once its bytes come.
  const landed = await until('its take', () => asset('spin'), (row) => (shown(row)?.metadata && !shown(row).absent && shown(row).frames) || row?.failure, 5 * STEP_MS);
  seen.take = { failure: landed.failure, main: shown(landed) };
  check('take_says_orbit', same(shown(landed)?.metadata?.orbit, ORBIT), shown(landed));
  check('take_counts_its_frames', shown(landed)?.frames === FRAMES, shown(landed)?.frames);
  check('project_db_holds_it', stored().some((record) => same(record.metadata?.video?.orbit, ORBIT)), stored());
  const versions = (await api(`${at}/assets/spin`)).body?.versions;
  check('versions_read_it', same(versions?.at(-1)?.files?.video?.metadata?.orbit, ORBIT), versions?.at(-1)?.files);

  // Open, it turns: no play button, a drag a quarter of its width is 90°, past 0° it wraps, an arrow steps 5°.
  await page.keyboard.press('f');
  await settled(page);
  await openBox(page, 'spin');
  const card = page.locator('.card.here[data-asset="spin"]');
  const stage = card.locator('.orbit');
  await stage.waitFor();
  await until('the clip loaded', () => stage.getAttribute('class'), (names) => names?.includes('ready'));
  check('no_play_button', (await card.locator('button.play').count()) === 0);
  const read = async () => ({ angle: Number(await stage.getAttribute('data-angle')), frame: Number(await stage.getAttribute('data-frame')), frames: Number(await stage.getAttribute('data-frames')) });
  const near = (angle, wanted) => Math.abs(angle - wanted) < 1.5;
  // `floor(angle / degrees × frames) mod frames`.
  const frameOf = (angle) => Math.floor((angle / 360) * FRAMES) % FRAMES;
  const box = await stage.boundingBox();
  const y = box.y + box.height / 2;
  const dragBy = async (from, by) => {
    await page.mouse.move(box.x + box.width * from, y);
    await page.mouse.down();
    for (let step = 1; step <= 8; step++) await page.mouse.move(box.x + box.width * (from + (by * step) / 8), y);
    await page.mouse.up();
    // Let go still: no coast.
    await page.waitForTimeout(400);
  };
  await page.emulateMedia({ reducedMotion: 'reduce' });
  // A width is a whole turn: 0.26 of it is 93.6°, inside frame 12's 7.5°.
  await dragBy(0.3, 0.26);
  const turned = await read();
  check('a_drag_turns_it', near(turned.angle, 93.6) && turned.frame === frameOf(turned.angle) && turned.frame === 12 && turned.frames === FRAMES, turned);
  const time = await until('the frame shown', () => stage.locator('video').evaluate((el) => el.currentTime), (t) => Math.abs(t - (turned.frame + 0.5) / 24) < 0.03).catch((why) => String(why));
  check('the_video_shows_that_frame', typeof time === 'number', time);
  await dragBy(0.8, -0.5);
  const wrapped = await read();
  check('it_wraps_past_0', near(wrapped.angle, 273.6) && wrapped.frame === frameOf(wrapped.angle), wrapped);
  await stage.focus();
  await page.keyboard.press('ArrowRight');
  const stepped = await read();
  check('an_arrow_steps_it', Math.abs(stepped.angle - wrapped.angle - 5) < 0.01 && stepped.frame === frameOf(stepped.angle), { wrapped, stepped });
  await shot('turned');

  // Off, it plays; Undo brings orbit viewing back.
  const toggle = card.locator('input[aria-label="Orbit: drag to turn"]');
  check('toggle_says_on', await toggle.isChecked());
  await toggle.click();
  await card.locator('button.play').waitFor();
  check('off_it_plays', (await stage.count()) === 0);
  const off = await until('off saved', () => asset('spin'), (row) => !shown(row)?.metadata?.orbit);
  check('off_saved', !shown(off)?.metadata && !stored().some((record) => record.metadata?.video?.orbit), stored());
  await page.locator('.note:has-text("Turned off orbit viewing") button:has-text("Undo")').first().click();
  await until('orbit back', () => asset('spin'), (row) => same(shown(row)?.metadata?.orbit, ORBIT));
  await stage.waitFor();
  check('undo_brings_it_back', (await card.locator('button.play').count()) === 0);
  const history = (await api(`${at}/history`)).body?.entries ?? [];
  check('history_says_it', history.some((entry) => entry.words === 'Turned off orbit viewing for take 1 of spin'), history.slice(0, 3).map((entry) => entry.words));

  // On for any video: a dropped one has no version, and gets one.
  await closeBox(page);
  await openBox(page, 'plain');
  const plain = page.locator('.card.here[data-asset="plain"]');
  await plain.locator('button.play').waitFor();
  await plain.locator('input[aria-label="Orbit: drag to turn"]').click();
  const on = await until('on saved', () => asset('plain'), (row) => same(shown(row)?.metadata?.orbit, ORBIT));
  await plain.locator('.orbit').waitFor();
  check('any_video_turns', (await plain.locator('button.play').count()) === 0 && on.versions === 1, { versions: on.versions, main: shown(on) });
  check('dropped_one_kept_in_db', stored().some((record) => same(record.metadata?.video?.orbit, ORBIT) && record.files?.video === shown(on)?.sha256), stored());
  await shot('plain');
  await closeBox(page);
  await shot('rest');
  check('badge_at_rest', (await boxOf(page, 'spin').locator('.orbit-badge').count()) === 1);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
