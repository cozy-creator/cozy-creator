// Undo and redo (Paul, 11-06): "an undo / redo history button ... helpful if I do something destructive and dumb by mistake". The project keeps
// the history, whoever edits it: on a real Calcifer, each of a delete, a merge, a tool taken off, words typed, a move, approve and unapprove and
// a take discarded is undone (Ctrl+Z, the top bar's Undo, a note's Undo) and redone (Ctrl+Shift+Z), shown at once and checked in project.db's
// boxes, canvas rows and takes after each. The API client's edit is undone from the page, the page's from the API; an undo a later edit would
// clobber is refused, saying which; and with Calcifer stopped, an edit and its undo wait, land in order when it's back, and the history is there
// after a restart, for Redo.
//   node undo.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $STOP: a shell command
// that stops this Calcifer, and $START one that brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execSync } = require('child_process');
const { existsSync, mkdtempSync, readFileSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectAction, calcifer, signIn, STEP_MS, boxOf, closeBox, launch, lastLook, lookAt, noStall, openBox, png, pointAt, settled, storedOf, until, rowsOf, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  const passed = ok === true;
  seen[name] = passed ? true : (detail ?? false);
  if (!passed) failures.push(name);
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
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-undo-${Date.now()}`);
  await page.getByLabel("Create without encryption", { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json', ...init.headers } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token });
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body.assets.find((row) => row.name === name);
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  // What the project holds, box by box, whatever their order: one put back goes at the end.
  const held = () => JSON.stringify(Object.entries(storedOf(folder)).sort(([a], [b]) => a.localeCompare(b)));
  // A box's fields as project.db stores them: none, it isn't there.
  const lines = (name) => storedOf(folder)[name] ?? null;
  const row = (name) => {
    const [held] = rowsOf(folder, `SELECT x, y FROM canvas WHERE name = '${name}'`);
    return held ? `${Math.round(held[0])},${Math.round(held[1])}` : null;
  };
  const edit = async (...words) => (await projectAction(folder, ...words)).value;
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
  /** Each check of a step: what project.db says, waited for until it says so, else what it says. */
  const says = async (name, read, wanted) => {
    let last;
    try {
      await until(name, async () => { last = await read(); return last; }, wanted);
      check(name, true);
    } catch (error) {
      check(name, false, { last: last ?? null, error: String(error) });
    }
    return last;
  };
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
  await says('delete_undone_in_the_file', () => lines('spare'), (held) => held?.approved !== undefined);
  await says('delete_undone_its_row', () => row('spare'), (held) => held === '1280,120');
  await redo();
  check('delete_redone_at_once', (await box('spare').count()) === 0);
  await says('delete_redone_in_the_file', () => lines('spare'), (held) => held === null);
  await undo();
  await says('spare_back', () => lines('spare'), Boolean);

  // A merge: green into red, one entry, one undo.
  const versionsOf = (held) => held ? { sha256: held.sha256 ?? null, versions: held.versions ?? null, prefixes: held.prefixes ?? null } : null;
  const beforeMerge = await api(at);
  const green = beforeMerge.body?.assets?.find((held) => held.name === 'green')?.sha256;
  const merge = seen.merge = { green_sha256: green ?? null, before_status: beforeMerge.status,
    red_before: versionsOf(beforeMerge.body?.assets?.find((held) => held.name === 'red')), requests: [] };
  if (beforeMerge.status !== 200 || typeof green !== 'string' || !green) throw new Error('merge fixture did not return the green source digest');
  const red = async () => {
    const answer = await api(at);
    return { status: answer.status, asset: versionsOf(answer.body?.assets?.find((held) => held.name === 'red')) };
  };
  const holds = (held) => held.status === 200 && Boolean(held.asset?.prefixes?.some((prefix) => prefix.main?.sha256 === green || green.includes(prefix.prefix)));
  const requests = new Map();
  const mergeRequest = (request) => {
    const path = new URL(request.url()).pathname;
    const method = request.method();
    if (!(method === 'POST' && path === `${at}/assets/red/versions`) && !(method === 'DELETE' && path === `${at}/assets/green`)) return;
    if (requests.size >= 16) { merge.requests_truncated = true; return; }
    const item = { method, path, request_ms: Date.now(), status: null, response_ms: null };
    if (method === 'POST') {
      try { item.source = request.postDataJSON()?.source ?? null; }
      catch (error) { item.body_error = String(error); }
    }
    requests.set(request, item);
    merge.requests.push(item);
  };
  const mergeResponse = (response) => {
    const item = requests.get(response.request());
    if (item) { item.status = response.status(); item.response_ms = Date.now(); }
  };
  const mergeFailed = (request) => {
    const item = requests.get(request);
    if (item) { item.failure = request.failure()?.errorText ?? null; item.failed_ms = Date.now(); }
  };
  page.on('request', mergeRequest);
  page.on('response', mergeResponse);
  page.on('requestfailed', mergeFailed);
  try {
    merge.drag_started_ms = Date.now();
    await drag('green', 'red');
    merge.drag_done_ms = Date.now();
    await says('merge_written', () => lines('green'), (held) => held === null);
    merge.history = await says('merge_one_entry', async () => (await history()).entries[0], (held) => held?.words === 'Merged green into red');
    merge.history_checked_ms = Date.now();
    merge.red_after = await says('merge_made_a_version', red, holds);
    merge.version_checked_ms = Date.now();
  } finally {
    page.off('request', mergeRequest);
    page.off('response', mergeResponse);
    page.off('requestfailed', mergeFailed);
  }
  await undo();
  check('merge_undone_at_once', (await box('green').count()) === 1);
  await says('merge_undone_in_the_file', () => lines('green'), Boolean);
  merge.red_after_undo = await says('merge_undone_its_version', red, (held) => held.status === 200 && Boolean(held.asset) && !holds(held));
  await redo();
  await says('merge_redone', () => lines('green'), (held) => held === null);
  await undo();
  await says('green_back', () => lines('green'), Boolean);

  // A tool taken off: its X beside the tool in Focus (a box at rest has no tool strip: cl-932), then the tool back, said under its picture
  // at rest at once, and off again.
  await openBox(page, 'tooled');
  await box('tooled').getByRole('button', { name: 'Remove image tool', exact: true }).click();
  await says('tool_off_written', () => lines('tooled'), (held) => held && held.tool === undefined);
  await undo();
  check('tool_back_at_once', (await box('tooled').locator('.node-tool').count()) === 1);
  await says('tool_undone_in_the_file', () => lines('tooled'), (held) => held?.tool === 'undo-e2e/still' && held.instructions?.includes('a lighthouse'));
  await redo();
  await says('tool_redone', () => lines('tooled'), (held) => held && held.tool === undefined);
  await undo();
  await says('tool_back', () => lines('tooled'), (held) => held?.tool === 'undo-e2e/still');

  // Words typed: one entry for all of it. Ctrl+Z in the words is theirs; outside them it's the project's.
  await openBox(page, 'words');
  const text = box('words').locator('.mention-text').first();
  await text.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('a stormy harbour');
  await closeBox(page);
  await says('words_written', () => lines('words'), (held) => held?.text?.includes('a stormy harbour'));
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
  await says('words_undone', () => lines('words'), (held) => held?.text?.includes('a quiet harbour'));
  await says('words_undone_shown', () => box('words').innerText(), (said) => said.includes('a quiet harbour'));
  await redo();
  await says('words_redone', () => lines('words'), (held) => held?.text?.includes('a stormy harbour'));

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
  const approvedLine = () => lines('shot')?.approved ?? null;
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
  // Unapproved as the CLI does it: on a picture a double tap only approves (06718eb3a; whether it should toggle is Paul's,
  // cl-908), and what's checked here is the undo of it.
  await edit('unapprove', 'shot');
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

  // The API client's edit, undone from the page's Undo button, which says whose it is.
  await edit('set', 'words', 'text=the cli was here');
  await says('cli_edit_written', () => lines('words'), (held) => held?.text?.includes('the cli was here'));
  const button = page.locator('.top .history [aria-label=Undo]');
  await says('undo_says_the_api', () => button.getAttribute('title'), (said) => said?.includes('by the API'));
  await button.click();
  await says('cli_edit_undone_from_the_page', () => lines('words'), (held) => held?.text?.includes('a stormy harbour'));

  // The page's edit, undone from the API.
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  await says('page_delete_written', () => lines('spare'), (held) => held === null);
  await until('listed', history, (held) => held.entries[0]?.words === 'Deleted spare' && held.entries[0]?.by.via === 'page');
  const undid = await edit('undo');
  check('page_edit_undone_from_the_api', undid.entries.some((entry) => entry.words === 'Deleted spare') && (await until('back', () => Promise.resolve(lines('spare')), Boolean).then(() => true, () => false)), undid);
  check('and_shown', await box('spare').waitFor({ timeout: 5000 }).then(() => true, () => false));

  // An undo a later edit would clobber is refused, naming that edit, and the project stays as the later edit left it.
  await away();
  await pointAt(page, box('spare').locator('.nm'));
  await box('spare').locator('.head [aria-label=Delete]').click();
  await says('deleted_again', () => lines('spare'), (held) => held === null);
  await edit('import', picture(60), 'spare');
  const theirs = await until('the API client\'s spare', () => asset('spare'), Boolean);
  await page.locator('.note:has-text("Deleted spare") button:has-text("Undo")').last().click();
  const refused = await until('refused', notes, (lines) => lines.some((line) => /Couldn't undo/.test(line))).catch(() => null);
  seen.refused = refused?.find((line) => /Couldn't undo/.test(line));
  check('conflicting_undo_refused', Boolean(seen.refused?.includes('spare is in the project again') && seen.refused.includes('Added spare')), refused);
  check('nothing_clobbered', (await asset('spare'))?.sha256 === theirs.sha256 && String(lines('spare')?.approved ?? '').startsWith(theirs.sha256.slice(7, 19)), lines('spare'));

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
  check('offline_file_as_before', held() === before, storedOf(folder));
  execSync(process.env.STOP, { shell: 'bash', stdio: 'ignore' });
  execSync(process.env.START, { shell: 'bash', stdio: 'ignore' });
  const kept = await edit('history', '-n', '3');
  check('history_survives_a_restart', kept.entries.some((entry) => entry.words === 'Deleted red' && entry.undone), kept);
  const again = await edit('redo');
  check('redo_after_a_restart', again.entries.some((entry) => entry.words === 'Deleted red') && lines('red') === null, again);
  check('page_follows_the_api', await until('red gone', () => box('red').count(), (n) => n === 0).then(() => true, () => false));

  // The same two history entries remain accessible through the unchanged Undo/Redo controls.
  await away();
  check('history_list_button_replaced_by_add_files', await page.locator('.top .history [aria-label="History"]').count() === 0 && await page.locator('.top .history [aria-label="Add files"]').count() === 1);
  await page.locator('.top .history [aria-label="Undo"]').click();
  await says('toolbar_undo_restores_red', () => lines('red'), Boolean);
  await page.locator('.top .history [aria-label="Undo"]').click();
  await says('toolbar_undo_removes_spare', () => lines('spare'), (spare) => spare === null);
  await page.locator('.top .history [aria-label="Redo"]').click();
  await says('toolbar_redo_restores_spare', () => lines('spare')?.approved ?? '', (spare) => String(spare).startsWith(theirs.sha256.slice(7, 19)));
  await page.locator('.top .history [aria-label="Redo"]').click();
  await says('toolbar_redo_deletes_red', () => lines('red'), (red) => red === null);
  check('history_in_project_db', rowsOf(folder, 'SELECT count(*) FROM ops')[0][0] > 0 && !existsSync(join(folder, '.history.json')) && !/history|undone/.test(JSON.stringify(storedOf(folder))));
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
