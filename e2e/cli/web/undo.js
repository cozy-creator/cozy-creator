// Undo and redo (Paul, 11-06): "an undo / redo history button ... helpful if I do something destructive and dumb by mistake". The project keeps
// the history, whoever edits it: on a real Calcifer, each of a delete, a merge, a tool taken off, words typed, a move, approve and unapprove and
// a take discarded is undone (Ctrl+Z, the top bar's Undo, a note's Undo) and redone (Ctrl+Shift+Z), shown at once and checked in project.yaml,
// its canvas rows and takes in project.db after each. The CLI's edit is undone from the page, the page's from the CLI; an undo a later edit would
// clobber is refused, saying which; and with Calcifer stopped, an edit and its undo wait, land in order when it's back, and the history is there
// after a restart, for `cozy project redo`.
//   node undo.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $STOP: a shell command
// that stops this Calcifer, and $START one that brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execSync, spawnSync } = require('child_process');
const { appendFileSync, existsSync, mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { calcifer, signIn, STEP_MS, boxOf, closeBox, launch, lastLook, lookAt, noStall, openBox, png, pointAt, settled, until, rowsOf, upload, yamlOf } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

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
  await page.goto(`${base}/#/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-undo-${Date.now()}`);
  await page.click('form button[type=submit]');
  await page.waitForURL(/#\/projects\/[^/]+$/);
  const id = page.url().match(/#\/projects\/([^/]+)/)[1];
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token });
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body.assets.find((row) => row.name === name);
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  const yaml = () => yamlOf(folder);
  // What the project holds, asset by asset, whatever order its lines are in: one put back goes at the end.
  const held = () => yaml().split(/\n(?=  \S)/).map((block) => block.trimEnd()).sort().join('\n')
  // An asset's own lines in project.yaml: none, it isn't there.
  const lines = (name) => yaml().match(new RegExp(`^  ${name}:.*(?:\n(?:    .*|\\s*$))*`, 'm'))?.[0].trimEnd() ?? null;
  const row = (name) => {
    const [held] = rowsOf(folder, `SELECT x, y FROM canvas WHERE name = '${name}'`);
    return held ? `${Math.round(held[0])},${Math.round(held[1])}` : null;
  };
  // What the CLI says, on either stream; refused, it throws.
  const cozy = (...words) => {
    const argv = ['project', '-p', folder, ...words];
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${argv.map((word) => `${word}\x1f`).join('')}\x1e`);
    const ran = spawnSync(process.env.COZY, argv, { encoding: 'utf8' });
    if (ran.status !== 0) throw new Error(`cozy project ${words.join(' ')}: ${ran.stderr}`);
    return `${ran.stdout}${ran.stderr}`;
  };
  const history = async () => (await api(`${at}/history?limit=50`)).body;
  const box = (name) => boxOf(page, name);
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());
  const notes = () => page.locator('.notes .note .line').allInnerTexts();
  const pills = () => page.locator('.top .pill').allInnerTexts();
  /** Nothing focused that keeps its own undo, and no box open. */
  const away = async () => {
    await closeBox(page);
    await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
    await page.mouse.move(2, 500);
  };
  const undo = async () => {
    await away();
    await page.keyboard.press('ControlOrMeta+z');
  };
  const redo = async () => {
    await away();
    await page.keyboard.press('ControlOrMeta+Shift+z');
  };
  /** Each check of a step: what the file says, waited for until it says so, else what it says. */
  const says = async (name, read, wanted) => check(name, await until(name, () => Promise.resolve(read()), wanted).then(() => true, () => read()));
  const place = async (name) => {
    const [, x, y] = (await box(name).evaluate((node) => node.style.transform)).match(/translate\((-?[\d.]+)px, *(-?[\d.]+)px\)/);
    return `${Math.round(x)},${Math.round(y)}`;
  };
  async function drag(name, onto) {
    await away();
    const from = await box(name).locator('.nm').boundingBox();
    const [x, y] = [from.x + 4, from.y + 4];
    const to = typeof onto === 'string' ? await box(onto).locator('.media').first().boundingBox().then((held) => ({ x: held.x + held.width / 2, y: held.y + held.height / 2 })) : { x: x + onto.dx, y: y + onto.dy };
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + 8, y + 2);
    await page.mouse.move(to.x, to.y, { steps: 8 });
    await page.waitForTimeout(100);
    await page.mouse.up();
  }

  // What's worked on, none of it rendering: pictures, one with two versions, a box of words, and one whose tool is only its name.
  const pictures = mkdtempSync(join(tmpdir(), 'web-undo-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const imported = async (name, shade, x, y) => api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(shade)), name, exact: true, at: { x, y } }) });
  await imported('shot', 10, 80, 120);
  await api(`${at}/assets/shot/versions`, { method: 'POST', body: JSON.stringify({ source: await upload(token, picture(20)) }) });
  await imported('red', 30, 480, 120);
  await imported('green', 40, 880, 120);
  await imported('spare', 50, 1280, 120);
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: '', name: 'words', exact: true, fields: { text: 'a quiet harbour' }, at: { x: 80, y: 560 } }) });
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: 'undo-e2e/still', kind: 'image', name: 'tooled', exact: true, fields: { instructions: 'a lighthouse' }, at: { x: 480, y: 560 } }) });
  for (const name of ['shot', 'red', 'green', 'spare', 'words', 'tooled']) await box(name).waitFor();
  await page.keyboard.press('f');
  await settled(page);
  check('undo_redo_in_the_top_bar_left', await page.evaluate(() => {
    const [undo, tabs] = [document.querySelector('.top .history [aria-label=Undo]'), document.querySelector('.top .ptabs')];
    return Boolean(undo && tabs && undo.getBoundingClientRect().right <= tabs.getBoundingClientRect().left && undo.getBoundingClientRect().left < 40);
  }));
  await shot('top-bar');

  // A delete: gone at once, back at once on Ctrl+Z, gone again on Ctrl+Shift+Z.
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  await says('delete_written', () => lines('spare'), (held) => held === null);
  await undo();
  check('delete_undone_at_once', (await box('spare').count()) === 1);
  await says('delete_undone_in_the_file', () => lines('spare'), (held) => held?.includes('approved:'));
  await says('delete_undone_its_row', () => row('spare'), (held) => held === '1280,120');
  await redo();
  check('delete_redone_at_once', (await box('spare').count()) === 0);
  await says('delete_redone_in_the_file', () => lines('spare'), (held) => held === null);
  await undo();
  await says('spare_back', () => lines('spare'), Boolean);

  // A merge: green into red, one entry, one undo.
  const green = (await asset('green')).sha256;
  const holds = async () => (await asset('red'))?.prefixes?.some((held) => held.main?.sha256 === green || green.includes(held.prefix)) ?? false;
  await drag('green', 'red');
  await says('merge_written', () => lines('green'), (held) => held === null);
  check('merge_one_entry', await until('the merge listed', history, (held) => held.entries[0]?.words === 'Merged green into red').then(() => true, async () => (await history()).entries[0]));
  check('merge_made_a_version', await until('green a version of red', holds, Boolean).then(() => true, () => false));
  await undo();
  check('merge_undone_at_once', (await box('green').count()) === 1);
  await says('merge_undone_in_the_file', () => lines('green'), Boolean);
  check('merge_undone_its_version', await until('that version gone', holds, (held) => !held).then(() => true, () => false));
  await redo();
  await says('merge_redone', () => lines('green'), (held) => held === null);
  await undo();
  await says('green_back', () => lines('green'), Boolean);

  // A tool taken off: its X, then the tool back, and off again.
  await away();
  await box('tooled').locator('button[aria-label="Take undo-e2e off tooled"]').click();
  await says('tool_off_written', () => lines('tooled'), (held) => held && !held.includes('tool:'));
  await undo();
  check('tool_back_at_once', (await box('tooled').locator('button[aria-label="Take undo-e2e off tooled"]').count()) === 1);
  await says('tool_undone_in_the_file', () => lines('tooled'), (held) => held?.includes('tool: undo-e2e/still') && held.includes('instructions: a lighthouse'));
  await redo();
  await says('tool_redone', () => lines('tooled'), (held) => held && !held.includes('tool:'));
  await undo();
  await says('tool_back', () => lines('tooled'), (held) => held?.includes('tool: undo-e2e/still'));

  // Words typed: one entry for all of it. Ctrl+Z in the words is theirs; outside them it's the project's.
  await openBox(page, 'words');
  const text = box('words').locator('.mention-text').first();
  await text.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('a stormy harbour');
  await closeBox(page);
  await says('words_written', () => lines('words'), (held) => held?.includes('a stormy harbour'));
  const entries = (await history()).entries.length;
  await openBox(page, 'words');
  await text.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' at night');
  await page.keyboard.press('ControlOrMeta+z');
  await page.waitForTimeout(500);
  check('ctrl_z_in_words_is_theirs', (await history()).entries.length === entries && !(await history()).entries.some((entry) => entry.undone && entry.words.startsWith('Set text')), (await history()).entries.slice(0, 3));
  await closeBox(page);
  await undo();
  await says('words_undone', () => lines('words'), (held) => held?.includes('a quiet harbour'));
  check('words_undone_shown', await until('shown', () => box('words').innerText(), (said) => said.includes('a quiet harbour')).then(() => true, () => box('words').innerText()));
  await redo();
  await says('words_redone', () => lines('words'), (held) => held?.includes('a stormy harbour'));

  // A move: back where it was, and where it went.
  const was = row('red');
  await drag('red', { dx: 150, dy: 90 });
  await says('move_written', () => row('red'), (held) => held && held !== was);
  const moved = row('red');
  await undo();
  check('move_undone_at_once', (await place('red')) === was, { now: await place('red'), was });
  await says('move_undone_in_the_file', () => row('red'), (held) => held === was);
  await redo();
  await says('move_redone', () => row('red'), (held) => held === moved);

  // Approve and unapprove, on an open box's takes.
  const approvedLine = () => lines('shot')?.match(/approved: (\S+)/)?.[1] ?? null;
  const first = approvedLine();
  const shotRow = await asset('shot');
  const other = shotRow.approved === 2 ? 1 : 2;
  await openBox(page, 'shot');
  await page.locator(`.card.here .take[data-take="${other}"]`).dblclick();
  await says('approve_written', approvedLine, (held) => held && held !== first);
  const approved = approvedLine();
  await undo();
  await says('approve_undone', approvedLine, (held) => held === first);
  await redo();
  await says('approve_redone', approvedLine, (held) => held === approved);
  await openBox(page, 'shot');
  await page.locator(`.card.here .take[data-take="${other}"]`).dblclick();
  await says('unapprove_written', approvedLine, (held) => held === null);
  await undo();
  await says('unapprove_undone', approvedLine, (held) => held === approved);

  // A take discarded, and its note's Undo, which is the history's.
  const takes = async () => (await asset('shot')).prefixes?.length;
  const had = await takes();
  await openBox(page, 'shot');
  await page.locator(`.card.here .take-at:has(.take[data-take="${other}"])`).hover();
  await page.locator(`.card.here .take-at:has(.take[data-take="${other}"]) .take-off`).click();
  check('discard_written', (await until('discarded', takes, (n) => n === had - 1).catch(() => null)) === had - 1 && rowsOf(folder, 'SELECT count(*) FROM versions WHERE discarded = 1')[0][0] > 0);
  await page.locator('.note:has-text("Discarded") button:has-text("Undo")').first().click();
  check('discard_undone_by_its_note', (await until('back', takes, (n) => n === had).catch(() => null)) === had);
  await redo();
  check('discard_redone', (await until('discarded again', takes, (n) => n === had - 1).catch(() => null)) === had - 1);
  await undo();
  check('discard_undone_again', (await until('back again', takes, (n) => n === had).catch(() => null)) === had);
  await closeBox(page);

  // The CLI's edit, undone from the page's Undo button, which says whose it is.
  cozy('set', 'words', 'text=the cli was here');
  await says('cli_edit_written', () => lines('words'), (held) => held?.includes('the cli was here'));
  const button = page.locator('.top .history [aria-label=Undo]');
  check('undo_says_the_cli', await until('the CLI named', () => button.getAttribute('title'), (said) => said?.includes('by the CLI')).then(() => true, () => button.getAttribute('title')));
  await button.click();
  await says('cli_edit_undone_from_the_page', () => lines('words'), (held) => held?.includes('a stormy harbour'));

  // The page's edit, undone from the CLI.
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  await says('page_delete_written', () => lines('spare'), (held) => held === null);
  await until('listed', history, (held) => held.entries[0]?.words === 'Deleted spare' && held.entries[0]?.by.via === 'page');
  const undid = cozy('undo');
  check('page_edit_undone_from_the_cli', undid.includes('Deleted spare') && (await until('back', () => Promise.resolve(lines('spare')), Boolean).then(() => true, () => false)), undid);
  check('and_shown', await box('spare').waitFor({ timeout: 5000 }).then(() => true, () => false));

  // An undo a later edit would clobber is refused, naming that edit, and the file stays as the later edit left it.
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  await says('deleted_again', () => lines('spare'), (held) => held === null);
  cozy('import', picture(60), 'spare');
  const theirs = await until('the CLI\'s spare', () => asset('spare'), Boolean);
  await page.locator('.note:has-text("Deleted spare") button:has-text("Undo")').last().click();
  const refused = await until('refused', notes, (lines) => lines.some((line) => /Couldn't undo/.test(line))).catch(() => null);
  seen.refused = refused?.find((line) => /Couldn't undo/.test(line));
  check('conflicting_undo_refused', Boolean(seen.refused?.includes('spare is in the project again') && seen.refused.includes('Added spare')), refused);
  check('nothing_clobbered', (await asset('spare'))?.sha256 === theirs.sha256 && (lines('spare') ?? '').includes(theirs.sha256.slice(7, 19)), lines('spare'));

  // Calcifer stopped: a delete and its undo, both shown at once and waiting; started again, both land in order, and the history keeps them.
  const before = held();
  execSync(process.env.STOP, { shell: 'bash', stdio: 'ignore' });
  await until('it says offline', pills, (lines) => lines.some((line) => line.startsWith('Calcifer offline')));
  await away();
  await pointAt(page, box('red').locator('.nm'));
  await box('red').locator('.head [aria-label=Delete]').click();
  check('offline_delete_shown', (await box('red').count()) === 0);
  await undo();
  check('offline_undo_shown', (await box('red').count()) === 1);
  const two = await until('two waiting', pills, (lines) => lines.some((line) => line.startsWith('Calcifer offline: 2 changes waiting'))).catch(pills);
  check('offline_both_wait', Array.isArray(two) && two.some((line) => line.includes('2 changes waiting')), two);
  execSync(process.env.START, { shell: 'bash', stdio: 'ignore' });
  await until('all saved', pills, (lines) => !lines.some((line) => /offline|Saving|Not saved/.test(line)));
  const landed = await until('the delete and its undo', history, (held) => held.entries.some((entry) => entry.words === 'Deleted red' && entry.undone)).catch(history);
  check('offline_landed_in_order', landed.entries?.some((entry) => entry.words === 'Deleted red' && entry.undone) && Boolean(lines('red')), landed.entries?.slice(0, 3));
  check('offline_file_as_before', held() === before, yaml());
  execSync(process.env.STOP, { shell: 'bash', stdio: 'ignore' });
  execSync(process.env.START, { shell: 'bash', stdio: 'ignore' });
  const kept = cozy('history', '-n', '3');
  check('history_survives_a_restart', /Deleted red \(undone/.test(kept), kept);
  const again = cozy('redo');
  check('redo_after_a_restart', again.includes('Deleted red') && lines('red') === null, again);
  check('page_follows_the_cli', await until('red gone', () => box('red').count(), (n) => n === 0).then(() => true, () => false));

  // Several back at once from the list, and forward again: the newest two are the CLI's redo of the delete and its import.
  await away();
  await page.locator('.top .history [aria-label=History]').click();
  const listed = page.locator('.history-list .history-row');
  await until('the list', () => listed.locator('.history-words').allInnerTexts(), (words) => words[0] === 'Deleted red' && words[1] === 'Added spare');
  seen.list = (await listed.locator('.history-words').allInnerTexts()).slice(0, 6);
  await shot('history-list');
  await listed.nth(1).click();
  await says('list_jumps_back_two', () => [lines('red'), lines('spare')], ([red, spare]) => Boolean(red) && spare === null);
  check('list_strikes_them_through', await until('struck through', () => page.locator('.history-list .history-row.undone').count(), (n) => n >= 2).then(() => true, () => false));
  await listed.nth(0).click();
  await says('list_goes_forward_again', () => [lines('red'), (lines('spare') ?? '').includes(theirs.sha256.slice(7, 19))], ([red, spare]) => red === null && spare);
  check('history_in_project_db', rowsOf(folder, 'SELECT count(*) FROM ops')[0][0] > 0 && !existsSync(join(folder, '.history.json')) && !/history|undone/.test(yaml()));
  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  // Whatever happened, Calcifer runs again for what comes after.
  try { execSync(process.env.START, { shell: 'bash', stdio: 'ignore' }); } catch {}
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
