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
const { projectFromPage, projectUrl, calcifer, signIn, STEP_MS, boxOf, closeBox, drop, launch, lastLook, lookAt, noStall, png, pointAt, until, rowsOf, upload } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok === true ? true : (detail ?? false);
  if (ok !== true) failures.push(name);
}
const WINDOW_MS = Number(process.env.UNDO_S ?? 40) * 1000;
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
  const token = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null')?.token);
  const api = async (path, init = {}) => {
    const answer = await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init.headers } });
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
  const imported = async (name, path, x) => api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(token, path), name, exact: true, at: { x, y: 560 } }) });

  // What reads it: a box of words, and a picture tool's prompt. Two boxes share one file, one of which is deleted later.
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: '', name: 'words', exact: true, fields: { text: '' }, at: { x: 80, y: 120 } }) });
  await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ tool: `${drawer}/draw`, kind: 'image', name: 'reader', exact: true, fields: { instructions: 'a hat' }, at: { x: 480, y: 120 } }) });
  const shared = picture('shared', 70);
  await imported('twin_a', shared, 80);
  await imported('twin_b', shared, 480);
  for (const name of ['words', 'reader', 'twin_a', 'twin_b']) await box(name).waitFor();

  // 1. A picture dropped: its own box.
  await drop(page, [{ name: 'pic.png', type: 'image/png', bytes: png(30) }], 1100, 300);
  await box('pic').waitFor();
  const pic = await until('pic held', () => asset('pic'), (held) => Boolean(held?.sha256));
  seen.pic = pic.sha256;
  await until('its thumbnail drawn', async () => thumbsOf(pic.sha256), (held) => held.length > 0).catch(() => []);
  seen.thumbs_before = thumbsOf(pic.sha256);
  check('dropped_file_in_files', inFiles(pic.sha256));

  // 2. Its box deleted. From here, what's asked for its picture is counted.
  const asked = [];
  page.on('request', (request) => { if (request.url().includes(hexOf(pic.sha256))) asked.push(new URL(request.url()).pathname); });
  await deleteBox('pic');
  await until('pic gone', () => asset('pic'), (held) => held === null);
  check('deleted_box_gone', (await box('pic').count()) === 0);

  // 3. `<` in another box offers the rest, not it.
  const words = box('words').locator('.mention-text[data-field="text"]');
  await words.click();
  await page.keyboard.type('<');
  const offered = await until('names offered', () => page.locator('.name-menu li').allTextContents(), (names) => names.length > 0).catch(() => []);
  seen.offered = offered;
  check('less_than_offers_the_rest', offered.includes('twin_b') && offered.includes('reader'), offered);
  check('less_than_never_offers_it', !offered.includes('pic'), offered);
  await page.keyboard.type('p');
  const pOffered = await page.locator('.name-menu li').allTextContents();
  check('typed_start_never_offers_it', !pOffered.includes('pic'), pOffered);
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');

  // 4. Typed anyway: marked deleted once saved, and a render of what mentions it, or the CLI's run of it, fails free, naming it.
  await page.keyboard.type('a picture of <pic> ');
  await away();
  const chip = box('words').locator('.chip[data-mention="pic"]');
  const marked = await until('its chip marked', async () => ({ count: await chip.count(), cls: await chip.first().getAttribute('class').catch(() => null), title: await chip.first().getAttribute('title').catch(() => null) }), (held) => held.count === 1 && /missing/.test(held.cls ?? '')).catch((why) => String(why));
  seen.marked = marked;
  check('mention_marked_deleted', marked?.title?.includes('pic was deleted') === true, marked);
  await shot('marked');
  await api(`${at}/assets/reader`, { method: 'PATCH', body: JSON.stringify({ fields: { instructions: 'a hat on <pic>' } }) });
  const runsBefore = (await api(`/v1/runs?project=${id}&limit=100`)).body?.runs?.length ?? null;
  await api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names: ['reader'] }) });
  const failed = await until('reader refused', () => asset('reader'), (held) => Boolean(held?.failure)).catch(() => null);
  seen.render_refused = failed?.failure ?? null;
  check('render_refused_naming_it_deleted', failed?.failure?.message?.includes('<pic> was deleted') === true, failed?.failure);
  check('render_refused_free', ((await api(`/v1/runs?project=${id}&limit=100`)).body?.runs?.length ?? null) === runsBefore && !(failed?.making?.length), { runsBefore, making: failed?.making });
  let cli = '';
  try {
    cli = execFileSync(process.env.COZY, ['run', `${drawer}/draw`, 'instructions=a hat on <pic>', '--on', 'local'], { cwd: folder, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: STEP_MS });
    check('cli_run_refused', false, cli);
  } catch (error) {
    cli = `${error.stdout ?? ''}${error.stderr ?? ''}`;
    check('cli_run_refused', /<pic> was deleted/.test(cli), cli.slice(-600));
  }

  // 5. No view shows its picture: no thumbnail under the words, no Files row, nothing of it asked for.
  check('no_thumbnail_under_the_words', (await box('words').locator('.mention-thumb').count()) === 0);
  await page.goto(await projectUrl(id, '/files'));
  const list = page.locator('.drawer .assetlist');
  await list.locator('.assetrow').first().waitFor();
  const rows = await list.locator('.assetrow').evaluateAll((all) => all.map((row) => row.dataset.asset));
  check('no_files_row', !rows.includes('pic') && rows.includes('twin_b'), rows);
  await page.goto(await projectUrl(id));
  await box('words').waitFor();
  await page.waitForTimeout(500);
  check('nothing_of_it_asked_for', asked.length === 0, asked);
  check('no_thumbnail_after_a_reload', (await box('words').locator('.mention-thumb').count()) === 0);

  // 6. Undo: back, offered, and its chip a picture again.
  await away();
  await page.keyboard.press('ControlOrMeta+z');
  await box('pic').waitFor();
  await until('pic back', () => asset('pic'), (held) => held?.sha256 === pic.sha256);
  await words.click();
  await page.keyboard.press('End');
  await page.keyboard.type('<');
  const again = await until('names offered again', () => page.locator('.name-menu li').allTextContents(), (names) => names.includes('pic')).catch(() => []);
  check('undo_offers_it_again', again.includes('pic'), again);
  await page.keyboard.press('Backspace');
  await away();
  const back = await until('its chip a picture', async () => ({ cls: await chip.first().getAttribute('class').catch(() => null), thumbs: await box('words').locator('.mention-thumb').count() }), (held) => !/missing/.test(held.cls ?? 'missing') && held.thumbs === 1).catch((why) => String(why));
  check('undo_resolves_its_mention', typeof back === 'object', back);
  await shot('undone');

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
  check('thumbnails_gone', thumbsOf(pic.sha256).length === 0, thumbsOf(pic.sha256));
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
  await imported('late', late, 880);
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
