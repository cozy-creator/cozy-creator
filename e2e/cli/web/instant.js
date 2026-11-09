// Local-first (Paul, 11-05): "frontend-state -> modified instantly; sends request to backend ... network responds, changes are confirmed". In a real
// Chrome on a real Calcifer, with the call held by a route that stalls it. A dropped tool's box is on the canvas in the frame it was dropped, with its
// POST held; what's typed and moved meanwhile lands after it; the box's own echo costs no read of the project and no redraw; an answer lost goes again
// under its key and makes one asset; another writer's name renames the box; a refused action rolls back with one line; a pending edit and another
// writer's end in the conflict line; a write's answer never claims another writer's lines; kill -9 of Calcifer mid-queue lands once; and a render whose
// answer is lost runs once.
//   node instant.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $RESTART: a shell command that kills this Calcifer with -9 and brings it
// back; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so it runs nothing.
// Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec, execFileSync, spawn } = require('child_process');
const { rmSync } = require('fs');
const { join } = require('path');
const { PYTHON, calcifer, carry, signIn, STEP_MS, boxOf, closeBox, importYaml, lastLook, lookAt, openBox, renderOf, until, yamlOf } = require('./page');

const failures = [];
const seen = {};
let during = 'the start';
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  during = name;
  if (!ok) failures.push(name);
}
const COZY = process.env.COZY;
const TOOL = join(__dirname, '..', 'project', 'tool.py');
const FIRST_RUN_MS = 300_000;
/** What it made besides its own project: its folders gone however it ends. */
let dropMade = async () => {};

/** Another writer of project.db (argv 1), as fast as a process can be: the moment box argv 2's field argv 3 is argv 4, it's argv 5 instead. */
const OUTSIDE = `
import json, sqlite3, sys, time
at, box, field, written, instead = sys.argv[1:]
db = sqlite3.connect(at, isolation_level=None, timeout=30)
print('watching', flush=True)
end = time.monotonic() + 10
while time.monotonic() < end:
    row = db.execute('SELECT value FROM fields WHERE box = ? AND field = ?', (box, field)).fetchone()
    if row and json.loads(row[0]) == written:
        db.execute('UPDATE fields SET value = ? WHERE box = ? AND field = ?', (json.dumps(instead), box, field))
        break
`;

/** A stand-in for a slow network: each call `match` names waits here until `release`. */
async function hold(page, method, path) {
  const calls = [];
  let open;
  const held = new Promise((done) => (open = done));
  const route = async (held_route) => {
    if (held_route.request().method() !== method) return held_route.fallback();
    calls.push(held_route.request());
    await held;
    await held_route.fallback().catch(() => {});
  };
  await page.route(path, route);
  return { calls, release: async () => { open(); await page.unroute(path, route); } };
}

/** Each call goes to Calcifer and its answer is lost, `times` times: what a dropped connection does after the write landed. */
async function loseAnswers(page, method, path, times) {
  const keys = [];
  let left = times;
  const route = async (lost) => {
    if (lost.request().method() !== method) return lost.fallback();
    keys.push(lost.request().headers()['idempotency-key']);
    if (left <= 0) return lost.fallback();
    left -= 1;
    await lost.fetch();
    await lost.abort('connectionreset');
  };
  await page.route(path, route);
  return { keys, done: () => page.unroute(path, route) };
}

(async () => {
  const base = calcifer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME, headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  context.setDefaultTimeout(STEP_MS);
  const page = await context.newPage();
  lookAt(page);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-instant-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const api = (path, init = {}) => page.evaluate(async ([path, init]) => {
    const token = JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token;
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, [path, init]);
  const folder = (await api(`/v1/projects/${id}`)).body.path;
  seen.project = { id, folder };
  // The project as `cozy project export` writes it, and its canvas as Calcifer answers it (cl-803: both are project.db's), asked from here: what
  // the page reads is counted.
  const yaml = () => yamlOf(folder);
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token);
  const cards = async () => (await (await fetch(`${base}/v1/projects/${id}/canvas`, { headers: { Authorization: `Bearer ${token}` } })).json()).cards ?? {};
  const cozy = (...words) => execFileSync(COZY, words, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  const status = () => JSON.parse(cozy('--json', 'project', '-p', folder, 'status')).assets;
  const hasAsset = (name) => new RegExp(`^  ${name}:`, 'm').test(yaml());
  const count = (name) => yaml().match(new RegExp(`^  ${name}(_\\d+)?:`, 'gm'))?.length ?? 0;
  const box = (name) => boxOf(page, name);
  const row = (label) => page.locator('.tools .tool', { has: page.locator(`b:text-is("${label}")`) }).first();
  await page.waitForSelector('.tools .tool');
  await page.waitForSelector('.empty-canvas');

  // The drop: a real drag of the Edit, which needs no Hub, its POST stalled. What a drop shows is timed in the page, by the frame.
  await page.evaluate(() => {
    const clock = (window.dropped = {});
    document.querySelector('.stage').addEventListener('letgo', () => {
      clock.drop = performance.now();
      clock.boxes = document.querySelectorAll('.card[data-asset]').length;
      requestAnimationFrame(() => {
        clock.frame = performance.now();
        clock.after = document.querySelectorAll('.card[data-asset]').length;
      });
    }, true);
  });
  const reads = [];
  page.on('request', (request) => {
    const { pathname, search } = new URL(request.url());
    if (request.method() === 'GET' && pathname.startsWith(`/v1/projects/${id}`)) reads.push(`${pathname.slice(`/v1/projects/${id}`.length) || '/'}${search}`);
  });
  const posts = await hold(page, 'POST', '**/v1/projects/*/assets');
  await carry(page, row('Edit'), { x: 1100, y: 800 });
  await until('the frame after the drop', () => page.evaluate(() => window.dropped.frame), Boolean);
  const dropped = await page.evaluate(() => window.dropped);
  seen.drop = { frame_ms: dropped.frame - dropped.drop, boxes: [dropped.boxes, dropped.after] };
  check('box_in_the_frame_of_the_drop', dropped.after === dropped.boxes + 1, seen.drop);
  const name = await page.locator('.card.landing').first().getAttribute('data-asset');
  check('box_named_before_any_answer', name === 'Edit' && !hasAsset(name), { name });
  check('post_is_held', posts.calls.length <= 1, posts.calls.length);

  // Moved and typed in while nothing is saved: each keeps its place in line, and nothing on screen says so.
  const head = await box(name).locator('.head').boundingBox();
  await page.mouse.move(head.x + 100, head.y + 8);
  await page.mouse.down();
  await page.mouse.move(head.x + 160, head.y + 48, { steps: 3 });
  await page.mouse.up();
  const place = () => box(name).evaluate((card) => getComputedStyle(card.closest('.react-flow__node')).transform);
  const moved = await place();
  check('nothing_saved_yet', !hasAsset(name) && !(name in (await cards())));
  check('no_spinner_on_it', (await box(name).locator('.status').count()) === 0, await box(name).innerText());
  reads.length = 0;
  await posts.release();
  await until('it landed', async () => hasAsset(name) && name in (await cards()), Boolean);
  await until('its move landed', async () => (await cards())[name]?.x, (x) => Number(x) > 0);
  await page.waitForTimeout(800);
  check('own_echo_moves_nothing', (await place()) === moved, { moved, now: await place() });
  check('own_echo_reads_no_whole_project', !reads.some((read) => read === '/' || read === '/canvas'), reads);
  seen.reads_after_a_drop = reads.slice();
  check('one_box', (await box(name).count()) === 1 && count(name) === 1, yaml());

  // An answer lost: the same key again, and one asset.
  const lost = await loseAnswers(page, 'POST', '**/v1/projects/*/assets', 2);
  await carry(page, row('Edit'), { x: 400, y: 900 });
  const second = `${name}_2`;
  await box(second).waitFor();
  await until('its answer came at last', () => Promise.resolve(lost.keys.length), (n) => n >= 3);
  await lost.done();
  await until('in the file', () => Promise.resolve(hasAsset(second)), Boolean);
  await page.waitForTimeout(400);
  check('retried_with_one_key', new Set(lost.keys).size === 1, lost.keys);
  check('retried_makes_one', count(name) === 2 && (await page.locator(`.card[data-asset^="${name}"]`).count()) === 2, yaml());

  // Another writer takes the name while this one's asked: the box takes what it got, and the other shows.
  const stalled = await hold(page, 'POST', '**/v1/projects/*/assets');
  await carry(page, row('Edit'), { x: 400, y: 200 });
  const third = `${name}_3`;
  await box(third).waitFor();
  cozy('project', '-p', folder, 'add', 'cozy/edit', third);
  await until('the other in the file', () => Promise.resolve(hasAsset(third)), Boolean);
  await stalled.release();
  await box(`${name}_4`).waitFor();
  await page.waitForTimeout(600);
  // What a writer elsewhere adds lands wherever it's free, which may be off screen.
  await page.keyboard.press('f');
  await page.waitForTimeout(400);
  const shown = await page.locator('.card[data-asset]').evaluateAll((cards) => cards.map((card) => card.dataset.asset));
  check('another_writers_name_renames_it', (await box(third).count()) === 1 && (await box(`${name}_4`).count()) === 1 && hasAsset(`${name}_4`), { shown, yaml: yaml() });

  // Refused: Calcifer holds no such file. The box goes, one line says why, and what's behind it in line goes on.
  const notes = () => page.locator('.notes .note .line').allInnerTexts();
  const boxes = await page.locator('.card[data-asset]').count();
  const refusing = await hold(page, 'POST', '**/v1/projects/*/imports');
  // A take let go there that this Calcifer never held, as a carry lets one go.
  await page.evaluate(() => {
    const take = { sha256: '0'.repeat(64), field: 'image', name: 'never_held' };
    document.elementFromPoint(900, 200).dispatchEvent(new CustomEvent('letgo', { bubbles: true, detail: { load: { take }, x: 900, y: 200 } }));
  });
  await box('never_held').waitFor({ timeout: 2000 });
  check('refused_box_shows_first', true);
  await refusing.release();
  await box('never_held').waitFor({ state: 'detached' });
  check('refused_rolls_back', (await page.locator('.card[data-asset]').count()) === boxes && !hasAsset('never_held'));
  const said = await until('one line', notes, (lines) => lines.some((line) => /Couldn't take out never_held/.test(line)));
  check('refused_says_one_line', said.filter((line) => /never_held/.test(line)).length === 1, said);

  // A pending edit and another writer's edit of the same field: the conflict line, and neither lost.
  cozy('project', '-p', folder, 'add', 'text', 'words', 'text=A storm.');
  await page.waitForTimeout(600);
  // Nothing held open: what's open lies over what's below it.
  await page.keyboard.press('f');
  await box('words').waitFor();
  await page.waitForTimeout(400);
  await openBox(page, 'words');
  await box('words').locator('.mention-text').click();
  const patches = await hold(page, 'PATCH', '**/v1/projects/*/assets/*');
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('mine');
  await page.mouse.click(1500, 1050);
  await until('its PATCH is held', () => Promise.resolve(patches.calls.length), (n) => n === 1);
  cozy('project', '-p', folder, 'set', 'words', 'text=theirs');
  await until('theirs in the file', () => Promise.resolve(yaml().includes('text: theirs')), Boolean);
  await page.waitForTimeout(600);
  await patches.release();
  await openBox(page, 'words');
  await until('the conflict line', () => box('words').getByRole('button', { name: 'Keep mine' }).count(), (n) => n > 0);
  check('conflict_keeps_both', yaml().includes('text: theirs') && (await box('words').locator('.mention-text').allInnerTexts()).join(' ').includes('mine'), await box('words').innerText());
  await box('words').getByRole('button', { name: 'Keep mine' }).click();
  await until('mine in the file', () => Promise.resolve(yaml().includes('text: mine')), Boolean);
  await closeBox(page);

  // Another program writing project.db the moment a write commits: the answer is for the write's own fields. Told the other's as its own, the page writes over them with no conflict.
  const outside = spawn(PYTHON, ['-c', OUTSIDE, join(folder, 'project.db'), 'words', 'text', 'ours', 'outside'], { stdio: ['ignore', 'pipe', 'ignore'] });
  const landed = new Promise((ended) => outside.once('exit', ended));
  await new Promise((watching) => outside.stdout.once('data', watching));
  const wrote = await api(`/v1/projects/${id}/assets/words`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ fields: { text: 'ours' } }) });
  await landed;
  const nowHeld = (await api(`/v1/projects/${id}`)).body.assets.find((asset) => asset.name === 'words');
  check('a_write_answers_for_its_own_lines', wrote.status === 200 && wrote.body.etag !== nowHeld.etag && yaml().includes('text: outside'), { wrote, etag: nowHeld.etag });

  // kill -9 with a box in line: it lands once, when Calcifer's back, with no reload.
  if (process.env.RESTART) {
    const waits = await hold(page, 'POST', '**/v1/projects/*/assets');
    await carry(page, row('Edit'), { x: 1100, y: 200 });
    const fifth = `${name}_5`;
    await box(fifth).waitFor();
    const restarted = new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
    await waits.release();
    await restarted;
    await until('it landed after the restart', () => Promise.resolve(hasAsset(fifth)), Boolean);
    await page.waitForTimeout(800);
    check('restart_mid_queue_lands_once', (await box(fifth).count()) === 1 && (yaml().match(new RegExp(`^  ${fifth}(_\\d+)?:`, 'gm'))?.length ?? 0) === 1 && !hasAsset(`${name}_6`), yaml());
    await until('the top bar says nothing waits', () => page.locator('.top .pill').allInnerTexts(), (lines) => !lines.some((line) => /offline|Saving|Not saved/.test(line)));
    check('nothing_left_saying_not_saved', true);
  }

  // A render whose answer is lost: the box says Queued at once, and however often it's asked, one run.
  if ((await api('/v1/status')).body.local_usable && !process.env.ALONE) {
    // Named before it's run, it's read by PyPI's Runtime, which knows no tool: the install a run makes on this machine's Runtime takes its place, as
    // tools.js primes its fixture, and the page knows it makes pictures.
    const stillRuns = async () => (await api(`/v1/runs?project=${id}&limit=50`)).body.runs.filter((run) => run.function === 'still' || /\/still$/.test(run.target ?? ''));
    execFileSync(COZY, ['--json', 'run', `${TOOL}/still`, 'instructions=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, env: { ...process.env, COZY_PROJECT: folder }, stdio: ['ignore', 'pipe', 'pipe'], timeout: FIRST_RUN_MS });
    const primed = (await stillRuns()).length;
    importYaml(folder, `${yaml().trimEnd()}\n  fixture:\n    tool: ${TOOL}/still\n    instructions: a whale\n`);
    await box('fixture').waitFor();
    await page.waitForTimeout(500);
    await openBox(page, 'fixture');
    const renders = await loseAnswers(page, 'POST', '**/v1/projects/*/runs', 1);
    await (await renderOf(page, 'fixture')).click();
    await page.waitForTimeout(20);
    // At once, the take it asks for is in the column beside it, saying it's queued: picked where its tool is known to make pictures
    // (51887d1b5), else with Queued under its words.
    check('render_says_queued_at_once', (await box('fixture').locator('.here-side .take.making[title="Queued"]').count()) === 1, await box('fixture').innerText());
    await until('a version of it', () => Promise.resolve(status().find((asset) => asset.name === 'fixture')?.versions ?? 0), (n) => n >= 1, FIRST_RUN_MS);
    // The take can land before the lost answer's reset reaches the page: what it asks again is seen before the route goes.
    await until('asked again', () => Promise.resolve(renders.keys.length), (n) => n >= 2, 10_000).catch(() => {});
    await renders.done();
    await page.waitForTimeout(1500);
    const held = status().find((asset) => asset.name === 'fixture');
    const runs = (await stillRuns()).length - primed;
    check('render_key_one', renders.keys.length >= 2 && new Set(renders.keys).size === 1, renders.keys);
    check('render_ran_once', held.versions === 1 && runs === 1, { versions: held.versions, runs });
    // Render keeps the box open: shut, as what follows begins at rest.
    await closeBox(page);
  }

  // Each project a tab, top left: one click, its boxes there at once, its own view kept. One that can't be read says why and what to do, on the canvas, with the tools
  // still there, and what's dropped on it comes back with one line.
  const made = async (word) => (await api('/v1/projects', { method: 'POST', body: JSON.stringify({ name: `web-instant-${word}-${Date.now()}` }), headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `${word}-${Date.now()}` } })).body;
  const [second_project, old] = [await made('tab'), await made('old')];
  dropMade = async () => {
    dropMade = async () => {};
    for (const project of [second_project, old]) rmSync(project.path, { recursive: true, force: true });
  };
  // One a newer cozy has written, its schema one this cozy doesn't read, as after a downgrade. Calcifer reads a schema as it opens a project, so
  // it's opened again by a Calcifer that starts after: on a shared computer, with no restart, that half isn't asked.
  const unreadable = Boolean(process.env.RESTART);
  if (unreadable) {
    execFileSync(PYTHON, ['-c', 'import sqlite3, sys; sqlite3.connect(sys.argv[1], isolation_level=None).execute("PRAGMA user_version = 99")', join(old.path, 'project.db')]);
    await new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
  }
  await page.reload();
  await page.waitForSelector('.ptabs .ptab.on');
  await box(name).waitFor();
  const tab = (project) => page.getByTitle(project.path, { exact: true });
  const here = page.locator('.ptab.on');
  await page.mouse.move(1400, 1000);
  await page.mouse.down();
  await page.mouse.move(1340, 960, { steps: 3 });
  await page.mouse.up();
  await page.waitForTimeout(300);
  const view = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const kept = await view();
  await tab(second_project).click();
  await page.locator('.ptab.on').and(page.getByTitle(second_project.path, { exact: true })).waitFor();
  await page.waitForSelector('.empty-canvas');
  check('a_tab_opens_its_project', (await box(name).count()) === 0, await here.innerText());
  await page.getByTitle(folder, { exact: true }).click();
  await box(name).waitFor({ timeout: 2000 });
  check('back_shows_its_boxes_at_once', true);
  await page.waitForTimeout(300);
  check('each_project_keeps_its_view', (await view()) === kept, { kept, now: await view() });
  if (unreadable) {
    await tab(old).click();
    const unread = page.locator('.unread');
    await unread.waitFor();
    check('what_cant_be_read_says_why', /schema 99/.test(await unread.innerText()) && /cozy update/.test(await unread.innerText()) && (await page.locator('.empty-canvas').count()) === 0, await unread.innerText());
    await until('the tools still there', () => row('Edit').count(), (n) => n === 1);
    await carry(page, row('Edit'), { x: 1200, y: 300 });
    await until('what was dropped on it came back', async () => (await notes()).filter((line) => /Couldn't place Edit/.test(line)).length, (n) => n === 1);
    check('a_drop_on_what_cant_be_read_rolls_back', (await page.locator('.card[data-asset]').count()) === 0);
    await page.getByTitle(folder, { exact: true }).click();
    await box(name).waitFor();
  }
  await dropMade();

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(async (error) => {
  console.error(`during ${during}:`, error);
  await dropMade().catch(() => {});
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
});
