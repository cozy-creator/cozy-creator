// Calcifer away (Paul, 11-06): "the UI should always remain responsive, even if calcifer is offline, and then we sync the changes in when calcifer
// comes back online". With Calcifer stopped: Turbo (a toggle under More) flips, a prompt is typed and saved twice, a take approved, a box moved twice, one renamed, one
// deleted and a picture dropped, each shown as it's done and still after a tab away and back, and the top bar counts what waits with nothing
// over the canvas. Started again with no reload: all of it is in project.db, in the order it was done, the top bar clears and nothing was refused.
//   node offline.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $STOP: a shell command that stops this Calcifer, and $START one that
// brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execFileSync, execSync } = require('child_process');
const { createHash } = require('crypto');
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, addBoxes, calcifer, signIn, STEP_MS, boxOf, closeBox, drop, lastLook, launch, lookAt, noStall, openBox, openCanvas, png, pointAt, rowsOf, storedOf, until, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}
const QUICK = join(__dirname, '..', 'project', 'quick.py');
/** What it made besides its own project: its folder gone however it ends. */
let dropOther = async () => {};
/** The first list of local code installs its Runtime to read it. */
const DESCRIBED_MS = 300_000;

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
  await page.fill('input[aria-label="New project\'s name"]', `web-offline-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const token = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token });
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body.assets.find((row) => row.name === name);
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  // What project.db holds, read past Calcifer, so it reads while Calcifer is away: a line a box, `name: field=value …`.
  const held = () => rowsOf(folder, "SELECT b.name, group_concat(f.field || '=' || f.value, ' ') FROM boxes b LEFT JOIN fields f ON f.box = b.key GROUP BY b.key ORDER BY b.ord")
    .map(([name, fields]) => `${name}: ${fields ?? ''}`).join('\n');
  const row = (name) => {
    const [found] = rowsOf(folder, `SELECT x, y FROM canvas WHERE name = '${name}'`);
    return found ? `${Math.round(found[0])},${Math.round(found[1])}` : null;
  };
  const box = (name) => boxOf(page, name);
  const place = async (name) => {
    const [, x, y] = (await box(name).evaluate((node) => node.style.transform)).match(/translate\((-?[\d.]+)px, *(-?[\d.]+)px\)/);
    return `${Math.round(x)},${Math.round(y)}`;
  };
  const pills = () => page.locator('.top .pill').allInnerTexts();
  const notes = () => page.locator('.notes .note .line').allInnerTexts();
  const away = async () => {
    await closeBox(page);
    await page.mouse.move(2, 500);
  };
  async function drag(name, dx, dy) {
    await away();
    const held = await box(name).locator('.nm').boundingBox();
    const [x, y] = [held.x + held.width / 2, held.y + held.height / 2];
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 2, y);
    await page.mouse.move(x + dx, y + dy, { steps: 4 });
    await page.mouse.up();
  }

  // What's worked on: a box of local code with its Turbo, a picture with two takes, and two more boxes. None runs anything.
  const pictures = mkdtempSync(join(tmpdir(), 'web-offline-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const imported = async (name, shade, x) => api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(shade), id), name, exact: true, at: { x, y: 120 } }) });
  await imported('shot', 10, 80);
  await api(`${at}/assets/shot/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(20), id) }) });
  await imported('named', 30, 480);
  await imported('spare', 40, 880);
  addBoxes(folder, [{ name: 'quick', tool: `${QUICK}/still`, kind: 'image', fields: { instructions: 'a lighthouse' } }]);
  // Named first, it's read by PyPI's Runtime, which knows no tool; the install a run makes on this machine's Runtime takes its place.
  if ((await api('/v1/status')).body.local_usable) execFileSync(process.env.COZY, ['--json', 'run', `${QUICK}/still`, 'instructions=primed', '--on', 'local'], { encoding: 'utf8', cwd: folder, stdio: ['ignore', 'pipe', 'inherit'], timeout: DESCRIBED_MS });
  await box('quick').waitFor();
  await page.keyboard.press('f');
  const turbo = box('quick').locator('input[type=checkbox][aria-label="Turbo"]');
  // Opened: in Focus every field of its tool is out, with no More (cl-938, aeddbfe14).
  const openQuick = async (timeout = STEP_MS) => {
    await openBox(page, 'quick');
    await turbo.waitFor({ timeout });
  };
  await openQuick(DESCRIBED_MS);
  await closeBox(page);
  // The open project's tab: its title is its folder, then what a click on it does (cl-958).
  const opened = (path) => page.locator('.ptab.on[aria-current="page"]').and(page.getByTitle(new RegExp(`^${path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\n`)));
  // A project as a person reaches it: by its tab, or from "N more" when the bar has no room for its tab (cl-877).
  const reach = async (path) => {
    const tab = page.locator('nav.ptabs a.ptab').and(page.getByTitle(path, { exact: true }));
    if (await tab.count()) await tab.click();
    else {
      await page.click('nav.ptabs .ptab.more');
      await page.locator('.drawer .list a').and(page.getByTitle(path, { exact: true })).click();
    }
    await opened(path).waitFor();
  };
  // Another project, to go to and come back from while Calcifer's away.
  const other = (await api('/v1/projects', { method: 'POST', body: JSON.stringify({ name: `web-offline-other-${Date.now()}`, unencrypted: true }), headers: { 'Idempotency-Key': `other-${Date.now()}` } })).body;
  dropOther = async () => rmSync(other.path, { recursive: true, force: true });
  // Read again, as the tabs list what's read at the start.
  await page.goto(await projectUrl(other.id));
  await page.reload();
  await opened(other.path).waitFor();
  await reach(folder);
  await box('quick').waitFor();
  const was = await asset('shot');
  const approve = was.pinned && was.approved === 2 ? 1 : 2;
  seen.before = { shot: { approved: was.approved, pinned: was.pinned }, quick: (await asset('quick')).tool };

  // Calcifer stops, and the page says so in its top bar and nowhere else.
  execSync(process.env.STOP, { shell: 'bash', stdio: 'ignore' });
  await until('it says offline', pills, (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
  check('offline_in_the_top_bar', (await page.locator('.gate, .unread, [role=dialog]').count()) === 0);

  // Each, done with nothing answering, shows as it's done.
  await openQuick();
  await turbo.click();
  check('turbo_flips_at_once', await turbo.isChecked());
  const prompt = box('quick').locator('.mention-text').first();
  for (const words of ['a lighthouse at dusk', 'a lighthouse at dawn']) {
    await prompt.click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.keyboard.type(words);
    await closeBox(page);
    await openBox(page, 'quick');
  }
  check('prompt_shows_what_was_typed', (await prompt.innerText()).trim() === 'a lighthouse at dawn', await prompt.innerText());
  await openBox(page, 'shot');
  await page.locator(`.card.here .take[data-take="${approve}"]`).dblclick();
  check('approve_shows_at_once', (await page.locator(`.card.here .take.approved[data-take="${approve}"]`).count()) === 1);
  // Moves are the canvas's: Focus is over it, so leave it first.
  await away();
  const from = await place('shot');
  await drag('shot', 140, 90);
  const moved = await place('shot');
  await drag('shot', 60, 120);
  const twice = await place('shot');
  check('moves_show_at_once', from !== moved && moved !== twice, { from, moved, twice });
  // A tap on its name at rest renames it (Focus has its own, by its heading).
  await away();
  await box('named').locator('.nm').click();
  await page.waitForSelector('.nm-field');
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('renamed');
  await page.keyboard.press('Enter');
  check('rename_shows_at_once', (await box('renamed').count()) === 1 && (await box('named').count()) === 0);
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  check('delete_shows_at_once', (await box('spare').count()) === 0);
  await away();
  const clear = await openCanvas(page);
  await drop(page, [{ name: 'late.png', type: 'image/png', bytes: png(77) }], clear.x, clear.y);
  await page.waitForSelector('.nm-field');
  await page.keyboard.press('Enter');
  check('drop_shows_at_once', (await box('late').count()) === 1);
  const shown = async () => ({ boxes: (await page.locator('.card[data-asset]').evaluateAll((cards) => cards.map((card) => card.dataset.asset))).sort().join(), shot: await place('shot') });
  const left = await shown();
  await reach(other.path);
  await reach(folder);
  await box('quick').waitFor();
  await page.waitForTimeout(500);
  const back = await shown();
  await openQuick();
  check('a_tab_away_and_back_keeps_them', back.boxes === left.boxes && back.shot === left.shot && (await turbo.isChecked()) && (await prompt.innerText()).trim() === 'a lighthouse at dawn', { left, back });
  await closeBox(page);

  // Counted, every one, while nothing has reached Calcifer: Turbo, two saves, the approval, the moves as one, the rename, the delete and the file.
  const eight = (lines) => lines.some((line) => line.startsWith('Calcifer offline: 8 changes waiting'));
  const waiting = await until('eight waiting', pills, eight, 5000).catch(pills);
  seen.said_offline = waiting;
  check('counts_what_waits', eight(waiting), waiting);
  check('nothing_written_while_away', !/turbo=true|dawn|dusk|^renamed:|^late:/m.test(held()) && /^spare:/m.test(held()), held());
  check('nothing_refused_while_away', !(await notes()).some((line) => /Couldn't/.test(line)), await notes());

  // Back with no reload: all of it lands, in its order, and the top bar clears.
  execSync(process.env.START, { shell: 'bash', stdio: 'ignore' });
  await until('all saved', pills, (lines) => !lines.some((line) => /offline|Saving|Not saved/.test(line)));
  await until('in project.db', () => Promise.resolve(held()), (text) => /turbo=true/.test(text) && /^late:/m.test(text) && /^renamed:/m.test(text) && !/^spare:/m.test(text));
  await until('the move in its row', () => Promise.resolve(row('shot')), (at) => at === twice);
  const quick = await asset('quick');
  const shot = await asset('shot');
  seen.after = { quick: { tool: quick.tool, instructions: quick.fields?.instructions }, shot: { approved: shot.approved, pinned: shot.pinned }, row: row('shot') };
  const stored = storedOf(folder);
  check('turbo_saved', quick.tool === `${QUICK}/still` && quick.fields?.turbo === true && stored.quick?.turbo === true, quick);
  check('prompt_saved_in_its_order', quick.fields?.instructions === 'a lighthouse at dawn' && stored.quick?.instructions === 'a lighthouse at dawn' && !JSON.stringify(stored).includes('dusk'), quick.fields);
  check('approve_saved', shot.pinned && shot.approved === approve, seen.after.shot);
  check('last_move_saved', row('shot') === twice && (await place('shot')) === twice, { row: row('shot'), twice });
  check('rename_saved', Boolean(await asset('renamed')) && !(await asset('named')), Object.keys(stored));
  check('delete_saved', !(await asset('spare')), Object.keys(stored));
  check('drop_saved', (await asset('late'))?.sha256 === `sha256:${createHash('sha256').update(png(77)).digest('hex')}`, await asset('late'));
  await openQuick();
  check('read_back_as_shown', (await turbo.isChecked()) && (await prompt.innerText()).trim() === 'a lighthouse at dawn');
  check('nothing_refused', !(await notes()).some((line) => /Couldn't/.test(line)), await notes());
  check('no_page_errors', errors.length === 0, errors);
  await dropOther();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  // Whatever happened, Calcifer runs again for what comes after.
  try { execSync(process.env.START, { shell: 'bash', stdio: 'ignore' }); } catch {}
  await dropOther().catch(() => {});
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
