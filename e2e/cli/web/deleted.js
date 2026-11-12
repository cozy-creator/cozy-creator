// cl-919 (Paul, 11-12): "When I delete a node, the image should be gone from the project." Paul's own steps, on a real Calcifer whose
// undo window is $UNDO_S seconds (config.toml's calcifer.undo_seconds, for tests): a picture dropped and its box deleted; `<` in another
// box doesn't offer it; its mention typed anyway is struck as deleted, a render of what mentions it and the CLI's run both fail free naming
// it, and no view shows its picture (no chip's thumbnail, no Files row). Undo: it resolves again. Deleted again with the page closed, once
// the window ends its file is gone from files/ and Calcifer's thumbnails, its Undo says it can't come back, and a file a box still shows
// stays. One deleted with Calcifer stopped across its window is gone once Calcifer starts.
//   node deleted.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $COZY: the CLI; $SHOT: where to keep its screenshots; $STOP: a shell
// command that stops this Calcifer, and $START one that brings it back. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { execSync, execFileSync } = require('child_process');
const { existsSync, mkdtempSync, readdirSync, writeFileSync } = require('fs');
const { homedir, tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, calcifer, signIn, STEP_MS, boxOf, closeBox, drop, launch, lastLook, lookAt, noStall, openBox, png, pointAt, renderOf, until, rowsOf, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}
const WINDOW_MS = Number(process.env.UNDO_S ?? 75) * 1000;
const drawer = join(__dirname, '..', 'project', 'draw.py');
const THUMBS = join(homedir(), '.cozy', 'cache', 'thumbs');

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  let page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-deleted-${Date.now()}`);
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
  const away = async () => {
    await closeBox(page);
    await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
    await page.mouse.move(2, 500);
  };
  const deleteBox = async (name) => {
    await away();
    await pointAt(page, box(name).locator('.nm'));
    await box(name).locator('.head [aria-label=Delete]').click();
  };
  const hexOf = (sha256) => sha256.replace(/^sha256:/, '');
  const inFiles = (sha256) => existsSync(folder) && readdirSync(join(folder, 'files')).some((name) => name.startsWith(hexOf(sha256)));
  const thumbsOf = (sha256) => (existsSync(THUMBS) ? readdirSync(THUMBS).filter((name) => name.startsWith(hexOf(sha256))) : []);
  const pictures = mkdtempSync(join(tmpdir(), 'web-deleted-'));
  const picture = (name, shade) => {
    const path = join(pictures, `${name}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  const imported = async (name, path, x) => api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, path, id), name, exact: true, at: { x, y: 560 } }) });

  // What reads it: a picture tool's prompt, and a box of words, clear of the tools on the left. Two boxes share one file, one of which is
  // deleted later.
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: `${drawer}/draw`, kind: 'image', name: 'reader', exact: true, fields: { instructions: 'a hat' }, at: { x: 480, y: 120 } }) });
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: '', name: 'words', exact: true, fields: { text: 'notes ' }, at: { x: 880, y: 120 } }) });
  const shared = picture('shared', 70);
  await imported('twin_a', shared, 480);
  await imported('twin_b', shared, 880);
  for (const name of ['words', 'reader', 'twin_a', 'twin_b']) await box(name).waitFor();

  // 1. A picture dropped: its own box.
  await drop(page, [{ name: 'pic.png', type: 'image/png', bytes: png(30) }], 1400, 300);
  await page.waitForSelector('.nm-field');
  await page.keyboard.press('Enter');
  await box('pic').waitFor();
  const pic = await until('pic held', () => asset('pic'), (held) => Boolean(held?.sha256));
  seen.pic = pic.sha256;
  await until('its thumbnail drawn', async () => thumbsOf(pic.sha256), (held) => held.length > 0).catch(() => []);
  seen.thumbs_before = thumbsOf(pic.sha256);
  check('dropped_file_in_files', inFiles(pic.sha256));

  // 2. Its box deleted as Paul did: a click opens it in Focus, where the ✕ on its picture is its only one. On the RC before cl-919 that
  // discarded its one version and left the box, empty, in the project (e2e 43159619369).
  await openBox(page, 'pic');
  await page.locator('[data-focused-asset="pic"] [aria-label="Discard version 1"]').click();
  await until('pic gone', () => asset('pic'), (held) => held === null).catch(() => null);
  check('deleted_box_gone', (await asset('pic')) === null && (await box('pic').count()) === 0, await asset('pic'));
  check('focus_left', await until('back on the canvas', () => Promise.resolve(page.url()), (url) => !url.includes('/assets/'), 5000).then(() => true, () => false), page.url());
  // From here, whatever is asked of its picture is counted: Focus drew it at several sizes before.
  await page.waitForTimeout(500);
  const asked = [];
  page.on('request', (request) => { if (request.url().includes(hexOf(pic.sha256))) asked.push(new URL(request.url()).pathname); });

  // 3. `<` in another box's prompt, a picture tool's, offers the rest, not it; nor does a box of words.
  const prompt = box('reader').locator('.mention-text[data-field=instructions]');
  const shown = box('reader').locator('.mention-thumb, .ghosts img');
  const openReader = async () => {
    await openBox(page, 'reader');
    await renderOf(page, 'reader');
    await prompt.waitFor();
  };
  const menu = () => until('names offered', () => page.locator('.name-menu li').allTextContents(), (names) => names.length > 0).catch(() => []);
  await openReader();
  await prompt.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('a hat on <');
  const offered = await menu();
  seen.offered = offered;
  check('less_than_offers_the_rest', offered.includes('twin_b'), offered);
  check('less_than_never_offers_it', !offered.includes('pic'), offered);
  await page.keyboard.type('p');
  const pOffered = await page.locator('.name-menu li').allTextContents();
  check('typed_start_never_offers_it', !pOffered.includes('pic'), pOffered);
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await away();
  await openBox(page, 'words');
  const words = box('words').locator('.mention-text[data-field="text"]');
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type('<');
  const wordsOffered = await menu();
  check('a_box_of_words_never_offers_it', wordsOffered.includes('twin_b') && !wordsOffered.includes('pic'), wordsOffered);
  await page.keyboard.press('Backspace');
  await away();

  // 4. Typed anyway: struck as deleted once saved, and a render of what mentions it, or the CLI's run of it, fails free, naming it.
  await openReader();
  await prompt.click();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.type('a hat on <pic> ');
  await away();
  await until('its words saved', async () => (await asset('reader'))?.fields?.instructions ?? '', (said) => said.includes('<pic>'));
  await openReader();
  const chip = prompt.locator('.chip[data-mention="pic"]');
  const marked = await until('its chip struck', async () => ({ count: await chip.count(), cls: await chip.first().getAttribute('class').catch(() => null), title: await chip.first().getAttribute('title').catch(() => null) }), (held) => held.count === 1 && /missing/.test(held.cls ?? '')).catch((why) => String(why));
  seen.marked = marked;
  check('mention_marked_deleted', marked?.title?.includes('pic was deleted') === true, marked);
  check('no_picture_behind_the_words', (await shown.count()) === 0);
  await shot('marked');
  await away();
  const runsBefore = (await api(`${at}/runs?limit=100`)).body?.runs?.length ?? null;
  seen.render_asked = await api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names: ['reader'], on: ['local'] }), headers: { 'Idempotency-Key': `deleted-${Date.now()}` } });
  const failed = await until('reader refused', () => asset('reader'), (held) => Boolean(held?.failure)).catch(() => null);
  seen.render_refused = failed?.failure ?? null;
  check('render_refused_naming_it_deleted', failed?.failure?.message?.includes('<pic> was deleted') === true, failed?.failure);
  check('render_refused_free', ((await api(`${at}/runs?limit=100`)).body?.runs?.length ?? null) === runsBefore && !(failed?.making?.length), { runsBefore, making: failed?.making });
  let cli = '';
  try {
    cli = execFileSync(process.env.COZY, ['run', `${drawer}/draw`, 'instructions=a hat on <pic>', '--on', 'local'], { cwd: folder, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: STEP_MS });
    check('cli_run_refused', false, cli);
  } catch (error) {
    cli = `${error.stdout ?? ''}${error.stderr ?? ''}`;
    check('cli_run_refused', /<pic> was deleted/.test(cli), cli.slice(-600));
  }

  // 5. No view shows its picture: no Files row, nothing of it asked for, and none behind the words after a reload.
  await page.goto(await projectUrl(id, '/files'));
  const list = page.locator('.drawer .assetlist');
  await list.locator('.assetrow').first().waitFor();
  const rows = await list.locator('.assetrow').evaluateAll((all) => all.map((row) => row.dataset.asset));
  check('no_files_row', !rows.includes('pic') && rows.includes('twin_b'), rows);
  await page.goto(await projectUrl(id));
  await box('reader').waitFor();
  await openReader();
  await page.waitForTimeout(500);
  check('no_picture_after_a_reload', (await shown.count()) === 0);
  check('nothing_of_it_asked_for', asked.length === 0, asked);

  // 6. Undo of the delete (the history's, as the page's Undo sends it; words typed since are newer): back, offered, and its mention a picture again.
  await away();
  const deleted = (await api(`${at}/history?limit=50`)).body.entries.find((held) => held.words === 'Deleted pic' && !held.undone);
  const undone = await api(`${at}/history/undo`, { method: 'POST', body: JSON.stringify({ entry: deleted?.id }) });
  check('undo_in_the_window', undone.status === 200, undone);
  await box('pic').waitFor();
  await until('pic back', () => asset('pic'), (held) => held?.sha256 === pic.sha256);
  await openReader();
  const back = await until('its chip a picture', async () => ({ cls: await chip.first().getAttribute('class').catch(() => null), shown: await shown.count() }), (held) => !/missing/.test(held.cls ?? 'missing') && held.shown > 0).catch((why) => String(why));
  check('undo_resolves_its_mention', typeof back === 'object', back);
  await prompt.click();
  await page.keyboard.press('End');
  await page.keyboard.type('<');
  const again = await menu();
  check('undo_offers_it_again', again.includes('pic'), again);
  await page.keyboard.press('Backspace');
  await shot('undone');
  await away();

  // Deleted again with the page closed, and a box sharing its file with another deleted: once the window ends, their files leave the
  // project unless a box still shows them, and Undo can't bring them back.
  await deleteBox('pic');
  await until('pic gone again', () => asset('pic'), (held) => held === null);
  await api(`${at}/assets/twin_a`, { method: 'DELETE' });
  const deletedAt = Date.now();
  await page.close();
  const purged = await until('its file gone', async () => inFiles(pic.sha256), (held) => !held, WINDOW_MS + 60_000).then(() => true, () => false);
  seen.purged_after_ms = Date.now() - deletedAt;
  check('file_gone_after_the_window', purged && seen.purged_after_ms >= WINDOW_MS - 2000, { purged, after_ms: seen.purged_after_ms, window_ms: WINDOW_MS });
  check('thumbnails_gone', await until('its thumbnails gone', async () => thumbsOf(pic.sha256), (held) => held.length === 0, 10_000).then(() => true, () => thumbsOf(pic.sha256)));
  check('no_record_of_it', rowsOf(folder, `SELECT count(*) FROM lock_files WHERE sha256 = '${pic.sha256}'`)[0][0] === 0);
  check('shared_file_stays', inFiles((await asset('twin_b')).sha256));
  const history = (await api(`${at}/history?limit=50`)).body.entries;
  const entry = history.find((held) => held.words === 'Deleted pic' && !held.undone);
  const refused = await api(`${at}/history/undo`, { method: 'POST', body: JSON.stringify({ entry: entry?.id }) });
  seen.undo_refused = refused.body;
  check('undo_says_it_cannot_come_back', refused.status === 409 && /can no longer be restored/.test(refused.body?.error?.message ?? ''), refused);
  check('undo_brings_nothing_back', (await asset('pic')) === null);
  const twin = await api(`${at}/history/undo`, { method: 'POST', body: JSON.stringify({ entry: history.find((held) => held.words === 'Deleted twin_a')?.id }) });
  check('shared_box_cannot_come_back_either', twin.status === 409, twin);

  // With Calcifer stopped across the window: gone once it starts.
  const late = picture('late', 110);
  await imported('late', late, 1280);
  const lateSha = (await until('late held', () => asset('late'), (held) => Boolean(held?.sha256))).sha256;
  await api(`${at}/assets/late`, { method: 'DELETE' });
  execSync(process.env.STOP, { shell: 'bash', stdio: 'ignore' });
  await new Promise((done) => setTimeout(done, WINDOW_MS + 3000));
  check('stopped_calcifer_left_it', inFiles(lateSha));
  execSync(process.env.START, { shell: 'bash', stdio: 'ignore' });
  check('caught_up_on_start', await until('late gone', async () => inFiles(lateSha), (held) => !held, 30_000).then(() => true, () => false));

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  try { execSync(process.env.START, { shell: 'bash', stdio: 'ignore' }); } catch {}
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
