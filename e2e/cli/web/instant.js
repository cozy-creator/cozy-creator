// Local-first (Paul, 11-05): "frontend-state -> modified instantly; sends request to backend ... network responds, changes are confirmed". In a real
// Chrome on a real Calcifer, with the call held by a route that stalls it. A dropped tool's box is on the canvas in the frame it was dropped, with its
// POST held; what's typed and moved meanwhile lands after it; the box's own echo costs no read of the project and no redraw; an answer lost goes again
// under its key and makes one asset; another writer's name renames the box; a refused action rolls back with one line; a pending edit and another
// writer's: the page's unsent words stand, then both merged by character, with nothing asked; a write's answer never claims another writer's lines; kill -9 of
// Calcifer mid-queue lands once; a box another writer adds is drawn where there's room in view; and a render whose answer is lost runs once.
//   node instant.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $RESTART: a shell command that kills this Calcifer with -9 and brings it
// back; $SHOT: where to keep its screenshots; $ALONE: on a shared computer, so it runs nothing.
// Prints what it saw as JSON and exits 1 if anything failed.
const { chromium } = require(process.env.PLAYWRIGHT);
const { exec, execFileSync } = require('child_process');
const { rmSync } = require('fs');
const { join } = require('path');
const { projectFolder, implementationRow, projectFromPage, projectJson, PYTHON, calcifer, carry, signIn, STEP_MS, boxOf, closeBox, addBoxes, lastLook, lookAt, openBox, renderOf, settled, storedOf, until } = require('./page');

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
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-instant-${Date.now()}`));
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const api = (path, init = {}) => page.evaluate(async ([path, init]) => {
    const token = JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null')?.token;
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, [path, init]);
  const folder = (await api(`/v1/projects/${id}`)).body.path;
  seen.project = { id, folder };
  // The project and its canvas as Calcifer answers them (cl-803: both are project.db's), asked from here: what the page reads is counted.
  const token = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null')?.token);
  const cards = async () => (await (await fetch(`${base}/v1/projects/${id}/canvas`, { headers: { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } })).json()).cards ?? {};
  const status = () => projectJson(folder).assets;
  const names = () => status().map((asset) => asset.name);
  const hasAsset = (name) => names().includes(name);
  const count = (name) => names().filter((held) => new RegExp(`^${name}(_\\d+)?$`).test(held)).length;
  // The box `words`'s text as project.db holds it.
  const words = () => storedOf(folder).words?.text ?? '';
  const box = (name) => boxOf(page, name);
  // The composition node, independent of any generative Edit Video operation.
  const editor = () => implementationRow(page, 'timeline', 'cozy/edit');
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
  await carry(page, await editor(), { x: 1100, y: 800 });
  await until('the frame after the drop', () => page.evaluate(() => window.dropped.frame), Boolean);
  const dropped = await page.evaluate(() => window.dropped);
  seen.drop = { frame_ms: dropped.frame - dropped.drop, boxes: [dropped.boxes, dropped.after] };
  check('box_in_the_frame_of_the_drop', dropped.after === dropped.boxes + 1, seen.drop);
  const name = await page.locator('.card.landing').first().getAttribute('data-asset');
  check('box_named_before_any_answer', Boolean(name) && !hasAsset(name), { name });
  check('a_pending_tool_drop_stays_in_Wander', await page.locator('[data-focused-asset]').count() === 0);
  // Its one POST is out and held: asked once, not yet (and not twice).
  await until('its POST asked', () => Promise.resolve(posts.calls.length), (n) => n >= 1, 5000).catch(() => {});
  check('post_is_held', posts.calls.length === 1, posts.calls.length);

  // Moved and typed in while nothing is saved: each keeps its place in line, and nothing on screen says so.
  const head = await box(name).locator('.head').boundingBox();
  await page.mouse.move(head.x + 100, head.y + 8);
  await page.mouse.down();
  await page.mouse.move(head.x + 160, head.y + 48, { steps: 3 });
  await page.mouse.up();
  const place = () => box(name).evaluate((card) => getComputedStyle(card.closest('.react-flow__node')).transform);
  const moved = await place();
  check('nothing_saved_yet', !hasAsset(name) && !(name in (await cards())));
  // Nothing on it says it's waiting: no status line, progress or queued/rendering dot (Card.tsx, StateDot.tsx).
  check('no_spinner_on_it', (await box(name).locator('.status, .prog, .media-making').count()) + (await box(name).getByRole('img', { name: /^(Queued|Rendering)$/ }).count()) === 0, await box(name).innerText());
  reads.length = 0;
  await posts.release();
  await until('it landed', async () => hasAsset(name) && name in (await cards()), Boolean);
  await until('its move landed', async () => (await cards())[name]?.x, (x) => Number(x) > 0);
  await page.waitForTimeout(800);
  check('own_echo_moves_nothing', (await place()) === moved, { moved, now: await place() });
  check('own_echo_reads_no_whole_project', !reads.some((read) => read === '/' || read === '/canvas'), reads);
  seen.reads_after_a_drop = reads.slice();
  check('one_box', (await box(name).count()) === 1 && count(name) === 1, names());

  // An answer lost: the same key again, and one asset.
  const lost = await loseAnswers(page, 'POST', '**/v1/projects/*/assets', 2);
  await carry(page, await editor(), { x: 400, y: 900 });
  const second = `${name}_2`;
  await box(second).waitFor();
  await until('its answer came at last', () => Promise.resolve(lost.keys.length), (n) => n >= 3);
  await lost.done();
  await until('in the project', () => Promise.resolve(hasAsset(second)), Boolean);
  await until('both on the page', () => page.locator(`.card[data-asset^="${name}"]`).count(), (n) => n === 2, 5000).catch(() => {});
  check('retried_with_one_key', new Set(lost.keys).size === 1, lost.keys);
  check('retried_makes_one', count(name) === 2 && (await page.locator(`.card[data-asset^="${name}"]`).count()) === 2, names());

  // Another writer takes the name while this one's asked: the box takes what it got, and the other shows.
  const stalled = await hold(page, 'POST', '**/v1/projects/*/assets');
  await carry(page, await editor(), { x: 400, y: 200 });
  const third = `${name}_3`;
  await box(third).waitFor();
  projectJson(folder, 'POST', '/assets', { tool: 'cozy/edit', name: third, exact: true, fields: {} });
  await until('the other in the project', () => Promise.resolve(hasAsset(third)), Boolean);
  await stalled.release();
  await box(`${name}_4`).waitFor();
  // What a writer elsewhere adds lands wherever it's free, which may be off screen.
  await page.keyboard.press('f');
  await settled(page);
  const shown = await page.locator('.card[data-asset]').evaluateAll((cards) => cards.map((card) => card.dataset.asset));
  check('another_writers_name_renames_it', (await box(third).count()) === 1 && (await box(`${name}_4`).count()) === 1 && hasAsset(`${name}_4`), { shown, names: names() });

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

  // A pending edit and another writer's edit of the same field: the page's unsent words stand, and then both are saved, merged by character, with
  // nothing asked (cl-904: Automerge text).
  projectJson(folder, 'POST', '/assets', { tool: '', name: 'words', exact: true, fields: { text: 'A storm.' } });
  // Nothing held open: what's open lies over what's below it.
  await page.keyboard.press('f');
  await box('words').waitFor();
  await settled(page);
  await openBox(page, 'words');
  await box('words').locator('.mention-text[data-field="text"]').click();
  const patches = await hold(page, 'PATCH', '**/v1/projects/*/assets/*');
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('mine');
  await page.mouse.click(1500, 1050);
  await until('its PATCH is held', () => Promise.resolve(patches.calls.length), (n) => n === 1);
  // The page's read of theirs, from the stream: what its unsent words must stand against.
  const theirsRead = page.waitForResponse((answer) => answer.request().method() === 'GET' && new URL(answer.url()).pathname.endsWith('/assets/words'), { timeout: 20_000 }).then(() => true, () => false);
  projectJson(folder, 'PATCH', '/assets/words', { fields: { text: 'theirs' } });
  await until('theirs in the project', () => Promise.resolve(words().startsWith('theirs')), Boolean);
  check('the_page_read_theirs', await theirsRead);
  await openBox(page, 'words');
  check('unsent_words_stand', (await box('words').locator('.mention-text[data-field="text"]').allInnerTexts()).join(' ').includes('mine'), await box('words').innerText());
  await patches.release();
  await until('both in the project', () => Promise.resolve(['minetheirs', 'theirsmine'].includes(words())), Boolean);
  // Nothing asked of the person (cl-904), whatever it would be called.
  check('nothing_asked', (await page.getByRole('alertdialog').count()) + (await box('words').getByRole('button', { name: /keep mine|keep theirs|take the file/i }).count()) === 0);
  await closeBox(page);

  // Another canonical writer commits after ours, before its delayed reply is consumed.
  // Keep the real response unchanged: its ETag must still describe only our write.
  during = 'a_write_answers_for_its_own_lines';
  const path = `${base}/v1/projects/${id}/assets/words`;
  const canonicalWords = () => projectJson(folder).assets.find((asset) => asset.name === 'words');
  let oursHeld, nowHeld, writerError;
  const order = { ours_recorded: false, outside_committed_before_reply: false };
  const delayReply = async (route) => {
    if (route.request().method() !== 'PATCH' || route.request().postDataJSON()?.fields?.text !== 'ours') return route.fallback();
    const response = await route.fetch();
    try {
      oursHeld = canonicalWords();
      order.ours_recorded = oursHeld?.fields?.text === 'ours' && words() === 'ours';
      if (!order.ours_recorded) throw new Error('the native PATCH did not commit our canonical text');
      projectJson(folder, 'PATCH', '/assets/words', { fields: { text: 'outside' } });
      nowHeld = canonicalWords();
      order.outside_committed_before_reply = nowHeld?.fields?.text === 'outside' && words() === 'outside';
    } catch (error) {
      writerError = error.message;
    } finally {
      await route.fulfill({ response });
    }
  };
  await page.route(path, delayReply);
  let wrote;
  try {
    wrote = await api(`/v1/projects/${id}/assets/words`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ fields: { text: 'ours' } }) });
  } finally {
    await page.unroute(path, delayReply);
  }
  check('a_write_answers_for_its_own_lines', !writerError && order.ours_recorded && order.outside_committed_before_reply && wrote.status === 200 && typeof oursHeld?.etag === 'string' && wrote.body.etag === oursHeld.etag && wrote.body.etag !== nowHeld?.etag && words() === 'outside', { wrote, ours_etag: oursHeld?.etag, outside_etag: nowHeld?.etag, order, writerError });

  // kill -9 with a box in line: it lands once, when Calcifer's back, with no reload.
  if (process.env.RESTART) {
    const waits = await hold(page, 'POST', '**/v1/projects/*/assets');
    await carry(page, await editor(), { x: 1100, y: 200 });
    const fifth = `${name}_5`;
    await box(fifth).waitFor();
    const restarted = new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
    await waits.release();
    await restarted;
    await until('it landed after the restart', () => Promise.resolve(hasAsset(fifth)), Boolean);
    // A while for a second landing, then all of it in view: the canvas draws only what's in view, and "once" counts what's drawn.
    await page.waitForTimeout(800);
    await page.keyboard.press('f');
    await settled(page);
    const shown = await box(fifth).count();
    check('restart_mid_queue_lands_once', shown === 1 && count(fifth) === 1 && !hasAsset(`${name}_6`), { shown, held: count(fifth), names: names() });
    await until('the top bar says nothing waits', () => page.locator('.top .pill').allInnerTexts(), (lines) => !lines.some((line) => /offline|Saving|Not saved/.test(line)));
    check('nothing_left_saying_not_saved', true);
  }

  // A render whose answer is lost: the box says Queued at once, and however often it's asked, one run.
  if ((await api('/v1/status')).body.local_usable && !process.env.ALONE) {
    // Named before it's run, it's read by PyPI's Runtime, which knows no tool: the install a run makes on this machine's Runtime takes its place, as
    // tools.js primes its fixture, and the page knows it makes pictures.
    const stillRuns = async () => (await api(`/v1/projects/${id}/runs?limit=50`)).body.runs.filter((run) => run.function === 'still' || /\/still$/.test(run.target ?? ''));
    execFileSync(COZY, ['--json', 'run', `${TOOL}/still`, 'instructions=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'pipe'], timeout: FIRST_RUN_MS });
    const primed = (await stillRuns()).length;
    addBoxes(folder, [{ name: 'fixture', tool: `${TOOL}/still`, kind: 'image', fields: { instructions: 'a whale' } }]);
    // Another writer's box, with no row: the page puts it where there's room in view, so it's drawn there (cl-972: it went below a fitted view with room).
    const drawn = await box('fixture').waitFor({ timeout: 15_000 }).then(() => true, () => false);
    const at = drawn ? await box('fixture').boundingBox() : null;
    check('another_writers_box_drawn_in_view', Boolean(at) && at.x >= 0 && at.y >= 0 && at.x < 1600 && at.y < 1100, { at, row: (await cards()).fixture });
    await box('fixture').waitFor();
    await openBox(page, 'fixture');
    const renders = await loseAnswers(page, 'POST', '**/v1/projects/*/runs', 1);
    await (await renderOf(page, 'fixture')).click();
    // At once: within a quarter second of the press, its take says it's on its way (StateDot's Queued or Rendering, or its making face).
    const onItsWay = box('fixture').getByRole('img', { name: /^(Queued|Rendering)$/ }).or(box('fixture').locator('.media-making, .prog, .here-side .take.making')).first();
    check('render_says_queued_at_once', await onItsWay.waitFor({ timeout: 250 }).then(() => true, () => false), await box('fixture').innerText());
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
  const made = async (word) => (await api('/v1/projects', { method: 'POST', body: JSON.stringify({ path: projectFolder(`web-instant-${word}-${Date.now()}`), unencrypted: true }), headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `${word}-${Date.now()}` } })).body;
  const [second_project, old] = [await made('tab'), await made('old')];
  dropMade = async () => {
    dropMade = async () => {};
    for (const project of [second_project, old]) rmSync(project.path, { recursive: true, force: true });
  };
  // One a newer cozy has written, its schema one this cozy doesn't read, as after a downgrade. Calcifer reads a schema as it opens a project, so
  // it's opened again by a Calcifer that starts after: on a shared computer, with no restart, that half isn't asked.
  const unreadable = Boolean(process.env.RESTART);
  let current;
  if (unreadable) {
    // What this cozy writes is the schema it reads: the refusal names that one.
    current = execFileSync(PYTHON, ['-c', 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute("PRAGMA user_version").fetchone()[0])', join(old.path, 'project.db')], { encoding: 'utf8' }).trim();
    execFileSync(PYTHON, ['-c', 'import sqlite3, sys; sqlite3.connect(sys.argv[1], isolation_level=None).execute("PRAGMA user_version = 99")', join(old.path, 'project.db')]);
    await new Promise((done, fail) => exec(process.env.RESTART, { shell: 'bash' }, (error) => (error ? fail(error) : done())));
  }
  await page.reload();
  await page.waitForSelector('.ptabs .ptab.on');
  await box(name).waitFor();
  const tab = (project) => page.getByTitle(project.path, { exact: true });
  // The open project's tab: its title is its folder, then what a click on it does (cl-958).
  const opened = (project) => page.locator('.ptab.on[aria-current="page"]').and(page.getByTitle(new RegExp(`^${project.path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\n`)));
  const here = page.locator('.ptab.on');
  await page.mouse.move(1400, 1000);
  await page.mouse.down();
  await page.mouse.move(1340, 960, { steps: 3 });
  await page.mouse.up();
  await settled(page);
  const view = () => page.evaluate(() => getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  const kept = await view();
  await tab(second_project).click();
  await opened(second_project).waitFor();
  await page.waitForSelector('.empty-canvas');
  check('a_tab_opens_its_project', (await box(name).count()) === 0, await here.innerText());
  await page.getByTitle(folder, { exact: true }).click();
  await box(name).waitFor({ timeout: 2000 });
  check('back_shows_its_boxes_at_once', true);
  await settled(page);
  check('each_project_keeps_its_view', (await view()) === kept, { kept, now: await view() });
  if (unreadable) {
    await tab(old).click();
    const unread = page.locator('.unread');
    await unread.waitFor();
    const refusal = await unread.innerText();
    check('what_cant_be_read_says_why', /schema 99/.test(refusal) && /^\d+$/.test(current) && refusal.includes(`only current CRDT schema ${current} is supported`) && /Preserve this database/.test(refusal) && /new current-format project in a separate empty directory/.test(refusal) && (await page.locator('.empty-canvas').count()) === 0, { refusal, current });
    await until('the Timeline implementation still there', async () => (await editor()).count(), (n) => n === 1);
    await carry(page, await editor(), { x: 1200, y: 300 });
    await until('what was dropped on it came back', async () => (await notes()).filter((line) => /Couldn't place Video Timeline/.test(line)).length, (n) => n === 1);
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
