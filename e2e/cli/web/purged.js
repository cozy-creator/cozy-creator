// cl-919: Undo after the purge. A deleted box keeps its files for the undo window (3 hours), then the purge removes them; the history still
// lists the delete. On a real Calcifer with its real window, the clock moved past it: two boxes deleted from the page, a tool's box with its
// prompt and a take and an imported picture; Calcifer stopped and project.db dated 3 h 1 min earlier (each delete and history entry), as if
// that long had gone; started again, the open purges both boxes' files. The page's Undo (Ctrl+Z) then brings each box back where it was
// without them, its tool and words kept, and says "<name>'s files were removed (deleted more than 3 hours ago)"; never "Couldn't undo".
// Redo deletes the bare box again and a second Undo brings it back again. A picture no one deleted keeps its file throughout.
//   node purged.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots. Prints what it saw as
// JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync } = require('child_process');
const { existsSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFolder, projectFromPage, projectUrl, calcifer, signIn, STEP_MS, PYTHON, boxOf, closeBox, launch, lastLook, lookAt, noStall, png, pointAt, rowsOf, until, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}
// Past the 3-hour window by a minute.
const AGO_MS = (3 * 60 + 1) * 60 * 1000;
const drawer = join(__dirname, '..', 'project', 'draw.py');

/** Calcifer stopped as a service manager stops it (a clean exit, not started again), and gone. */
function stop() {
  const pid = JSON.parse(readFileSync(process.env.HOLDER, 'utf8')).pid;
  try { process.kill(pid, 'SIGTERM'); } catch {}
  const deadline = Date.now() + STEP_MS;
  for (;;) {
    try { process.kill(pid, 0); } catch { return; }
    if (Date.now() > deadline) throw new Error(`Calcifer ${pid} outlived its SIGTERM`);
    execFileSync('sleep', ['0.2']);
  }
}
/** Calcifer started again, as any verb that needs it starts it. */
const start = () => execFileSync(process.env.COZY, ['machine', 'ls'], { stdio: 'ignore', timeout: STEP_MS });

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-purged-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const token = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(`${at}/assets/${name}`)).body?.asset ?? null;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  const box = (name) => boxOf(page, name);
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  const notes = () => page.locator('.notes .note .line').allInnerTexts();
  const away = async () => {
    await closeBox(page);
    await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
    await page.mouse.move(2, 500);
  };
  const press = async (keys) => {
    await away();
    await page.keyboard.press(keys);
  };
  const deleteBox = async (name) => {
    await away();
    await pointAt(page, box(name).locator('.nm'));
    await box(name).locator('.head [aria-label=Delete]').click();
    await until(`${name} deleted`, () => asset(name), (held) => held === null);
  };
  const hexOf = (sha256) => sha256.replace(/^sha256:/, '');
  const inFiles = (sha256) => existsSync(join(folder, 'files')) && readdirSync(join(folder, 'files')).some((name) => name.startsWith(hexOf(sha256)));
  const pictures = mkdtempSync(join(tmpdir(), 'web-purged-'));
  const picture = (name, shade) => {
    const path = join(pictures, `${name}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const place = (name) => {
    const [held] = rowsOf(folder, `SELECT x, y FROM canvas WHERE name = '${name}'`);
    return held ? `${Math.round(held[0])},${Math.round(held[1])}` : null;
  };
  /** The note an Undo or Redo of `name` left, once its answer is in: one that says its files were removed, or a refusal. */
  const said = (name) => until(`${name}'s note`, notes, (lines) => lines.some((line) => line.includes(`${name}'s files were removed`) || /Couldn't/.test(line)))
    .then((lines) => lines.find((line) => line.includes(`${name}'s files were removed`) || /Couldn't/.test(line)), () => null);

  // A tool's box with its prompt and a take, an imported picture, and a picture no one deletes.
  const prompt = 'a red hat on a cat';
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: `${drawer}/draw`, kind: 'image', name: 'hero', exact: true, fields: { instructions: prompt }, at: { x: 480, y: 120 } }) });
  await api(`${at}/assets/hero/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture('hero', 30), id) }) });
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture('pic', 90), id), name: 'pic', exact: true, at: { x: 880, y: 120 } }) });
  await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture('stays', 150), id), name: 'stays', exact: true, at: { x: 1280, y: 120 } }) });
  const hero = await until('hero holds its take', () => asset('hero'), (held) => Boolean(held?.sha256));
  const pic = await until('pic held', () => asset('pic'), (held) => Boolean(held?.sha256));
  const stays = await until('stays held', () => asset('stays'), (held) => Boolean(held?.sha256));
  for (const name of ['hero', 'pic', 'stays']) await box(name).waitFor();
  seen.files = { hero: hero.sha256, pic: pic.sha256, stays: stays.sha256 };
  seen.places = { hero: place('hero'), pic: place('pic') };
  check('takes_in_files', inFiles(hero.sha256) && inFiles(pic.sha256) && inFiles(stays.sha256));

  // Both deleted from the page, hero first.
  await deleteBox('hero');
  await deleteBox('pic');
  check('files_kept_inside_the_window', inFiles(hero.sha256) && inFiles(pic.sha256));

  // The clock past the window: Calcifer stopped, and every delete and history entry of the project dated 3 h 1 min earlier. Its real window:
  // config.toml says no undo_seconds.
  stop();
  const aged = execFileSync(PYTHON, ['-c', [
    'import sqlite3, sys',
    'db = sqlite3.connect(sys.argv[1])',
    'ago = int(sys.argv[2])',
    "rows = db.execute(\"UPDATE lock_assets SET record = json_set(record, '$.deleted_ms', json_extract(record, '$.deleted_ms') - ?) WHERE json_extract(record, '$.deleted_ms') IS NOT NULL\", (ago,)).rowcount",
    "ops = db.execute('UPDATE ops SET at_ms = at_ms - ?', (ago,)).rowcount",
    'db.commit()',
    'print(rows, ops)',
  ].join('\n'), join(folder, 'project.db'), String(AGO_MS)], { encoding: 'utf8' }).trim().split(' ').map(Number);
  seen.aged = { deletes: aged[0], entries: aged[1] };
  check('both_deletes_dated_back', aged[0] === 2 && aged[1] >= 5, seen.aged);
  start();

  // Its open purges them.
  await page.goto(await projectUrl(id));
  await box('stays').waitFor();
  const purged = await until('their files gone', async () => inFiles(hero.sha256) || inFiles(pic.sha256), (held) => !held, 30_000).then(() => true, () => false);
  check('files_purged_past_the_window', purged, { hero: inFiles(hero.sha256), pic: inFiles(pic.sha256) });
  check('a_box_still_there_keeps_its_file', inFiles(stays.sha256));
  const history = (await api(`${at}/history?limit=50`)).body;
  seen.history = history.entries.map((entry) => entry.words);
  check('history_still_lists_the_deletes', ['Deleted pic', 'Deleted hero'].every((words) => history.entries.some((entry) => entry.words === words && !entry.undone)), seen.history);

  // Undo: pic, the newest, back where it was without its file, and said so.
  await press('ControlOrMeta+z');
  const picSaid = await said('pic');
  seen.pic_said = picSaid;
  check('undo_says_its_files_were_removed', picSaid?.includes("pic's files were removed (deleted more than 3 hours ago)") === true, picSaid);
  await box('pic').waitFor().catch(() => {});
  const picBack = await until('pic back', () => asset('pic'), (held) => held !== null).catch(() => null);
  check('pic_back_without_its_file', picBack !== null && !picBack.sha256 && !picBack.versions, picBack);
  check('pic_back_in_its_place', place('pic') === seen.places.pic, place('pic'));
  await shot('pic-back');

  // Undo again: hero, its tool and prompt kept, none of its takes.
  await press('ControlOrMeta+z');
  const heroSaid = await said('hero');
  seen.hero_said = heroSaid;
  check('undo_says_heros_files_were_removed', heroSaid?.includes("hero's files were removed (deleted more than 3 hours ago)") === true, heroSaid);
  const heroBack = await until('hero back', () => asset('hero'), (held) => held !== null).catch(() => null);
  check('hero_back_with_its_tool_and_words', heroBack?.fields?.instructions === prompt && /draw/.test(heroBack?.tool ?? ''), heroBack && { tool: heroBack.tool, fields: heroBack.fields });
  check('hero_back_without_its_take', heroBack !== null && !heroBack.sha256 && !heroBack.versions, heroBack && { sha256: heroBack.sha256, versions: heroBack.versions });
  check('hero_back_in_its_place', place('hero') === seen.places.hero, place('hero'));
  await box('hero').waitFor().catch(() => {});

  // Redo deletes the bare box again, and Undo brings it back again: the history holds it as it came back.
  await press('ControlOrMeta+Shift+z');
  check('redo_deletes_it_again', await until('hero gone again', () => asset('hero'), (held) => held === null, 15_000).then(() => true, () => false));
  await press('ControlOrMeta+z');
  check('undo_again_brings_it_back', await until('hero back again', () => asset('hero'), (held) => held?.fields?.instructions === prompt, 15_000).then(() => true, () => false));
  check('no_file_came_back', !inFiles(hero.sha256) && !inFiles(pic.sha256));

  // A reload shows both, with nothing refused on the way.
  await page.reload();
  for (const name of ['hero', 'pic', 'stays']) await box(name).waitFor().catch(() => {});
  check('both_on_the_canvas_after_a_reload', (await box('hero').count()) === 1 && (await box('pic').count()) === 1);
  const lines = await notes();
  seen.notes = lines;
  check('never_couldnt_undo', !seen.pic_said?.includes("Couldn't") && !seen.hero_said?.includes("Couldn't") && !lines.some((line) => /Couldn't/.test(line)), lines);
  await shot('back');

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  try { start(); } catch {}
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
