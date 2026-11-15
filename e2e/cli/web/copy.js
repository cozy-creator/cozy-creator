// Copy, paste and duplicate boxes as in Figma (cl-924, Paul 11-12), in a real browser on a real Calcifer. Two boxes selected, Ctrl+C and Ctrl+V
// with the pointer on the canvas: two new boxes there with names of their own, the same tool, words and pictures (the same versions and approval,
// the same files by hash, no bytes copied), `<self.1>` the copy's own, a mention between them naming the copy and another unchanged, in one entry
// of the history that Undo takes back whole. With the pointer off the canvas they land beside what was copied. Ctrl+D, Alt and a drag, and
// Duplicate on a box's menu copy too; Ctrl+V in a box's words pastes only words.
//   node copy.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, readdirSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, boxOf, calcifer, closeBox, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, projectUrl, settled, signIn, STEP_MS, until, upload } = require('./page');

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

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = { token: holderToken() };
  const api = (path, init = {}) => page.evaluate(async ({ path, init }) => {
    const answer = await fetch(path, { ...init, headers: { 'cozy-project-storage': '3', 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init });
  const post = (path, body) => api(path, { method: 'POST', body: JSON.stringify(body) });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-copy-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const state = async () => (await api(at)).body;
  const asset = async (name) => (await state())?.assets?.find((row) => row.name === name);
  const names = async () => ((await state())?.assets ?? []).map((row) => row.name).sort();
  const rows = async () => (await api(`${at}/canvas`)).body?.cards ?? {};
  const folder = (await state()).path;
  seen.project = { id, folder };
  /** Each version as its number and its files' hashes, and the one approved. */
  const versionsOf = async (name) => {
    const read = (await api(`${at}/assets/${encodeURIComponent(name)}`)).body;
    return { approved: read?.asset?.approved ?? null, sha256: read?.asset?.sha256 ?? null, versions: (read?.versions ?? []).map((version) => `${version.number}:${Object.values(version.files).map((file) => file.sha256).sort().join('+')}`) };
  };
  const bytesHeld = () => readdirSync(join(folder, 'files')).length;

  // hei: a picture with two versions and BiRefNet on it, reading `<self.1>`; words: a text naming <hei.2> and <other>; other: a picture.
  const pictures = mkdtempSync(join(tmpdir(), 'web-copy-'));
  const picture = (shade) => {
    const path = join(pictures, `${shade}.png`);
    writeFileSync(path, png(shade));
    return path;
  };
  await post(`${at}/imports`, { source: await upload(held?.token, picture(40), id), name: 'hei', exact: true, at: { x: 100, y: 100 } });
  await post(`${at}/assets/hei/versions`, { source: await upload(held?.token, picture(80), id) });
  await post(`${at}/imports`, { source: await upload(held?.token, picture(120), id), name: 'other', exact: true, at: { x: 1400, y: 100 } });
  const searched = (await api(`/v1/tools?project=${id}&q=birefnet`)).body?.tools ?? [];
  const cutout = searched.find((tool) => tool.package === 'birefnet' && tool.makes === 'image');
  seen.birefnet_tool = cutout?.id ?? searched.map((tool) => tool.id);
  if (!cutout) throw new Error('the Hub lists no BiRefNet tool');
  const put = await api(`${at}/assets/hei`, { method: 'PATCH', body: JSON.stringify({ tool: cutout.id }) });
  if (put.status !== 200) throw new Error(`BiRefNet on hei: ${put.status} ${JSON.stringify(put.body)}`);
  const words = 'a lighthouse in <hei.2> beside <other>';
  const wrote = await post(`${at}/assets`, { tool: '', name: 'words', exact: true, kind: 'text', fields: { text: words }, at: { x: 100, y: 700 } });
  if (wrote.status !== 201) throw new Error(`words: ${wrote.status} ${JSON.stringify(wrote.body)}`);
  const hei = await until('BiRefNet on hei', () => asset('hei'), (row) => row?.tool === cutout.id);
  const heiVersions = await versionsOf('hei');
  seen.hei = { fields: hei.fields, ...heiVersions };
  check('hei_reads_itself_with_two_versions', hei.fields?.image === '<self.1>' && heiVersions.versions.length === 2, seen.hei);
  const otherFile = (await asset('other'))?.sha256;
  const held0 = bytesHeld();

  for (const name of ['hei', 'words', 'other']) await boxOf(page, name).waitFor();
  await fitCanvas(page);
  const quiet = async () => {
    await page.evaluate(() => {
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      document.getSelection()?.removeAllRanges();
    });
  };
  const selected = () => page.evaluate(() => [...document.querySelectorAll('.react-flow__node.selected .card')].map((card) => card.dataset.asset).sort());
  /** Only `names` selected, by Shift and a click on each one's name, as a person adds to a selection. */
  const pick = async (...picked) => {
    await quiet();
    await page.keyboard.press('Escape');
    await until('nothing selected', selected, (now) => now.length === 0);
    for (const name of picked) await boxOf(page, name).locator('.nm').first().click({ modifiers: ['Shift'] });
    return until(`${picked.join(' and ')} selected`, selected, (now) => now.join() === [...picked].sort().join());
  };
  /** A point on the canvas with nothing on it. */
  const empty = () => page.evaluate(() => {
    const pane = document.querySelector('.react-flow__pane').getBoundingClientRect();
    for (let y = pane.bottom - 80; y > pane.top + 80; y -= 30) {
      for (let x = pane.right - 120; x > pane.left + 80; x -= 30) {
        if (document.elementFromPoint(x, y)?.classList.contains('react-flow__pane')) return { x, y };
      }
    }
    return null;
  });
  /** Where a point on the screen is on the canvas. */
  const onCanvas = (point) => page.evaluate(({ x, y }) => {
    const pane = document.querySelector('.react-flow__pane').getBoundingClientRect();
    const [, tx, ty, zoom] = document.querySelector('.react-flow__viewport').style.transform.match(/translate\((-?[\d.]+)px, *(-?[\d.]+)px\) scale\(([\d.]+)\)/).map(Number);
    return { x: (x - pane.left - tx) / zoom, y: (y - pane.top - ty) / zoom };
  }, point);
  const shows = (name) => until(`${name} shows its picture`, () => boxOf(page, name).locator('img').evaluateAll((images) => images.some((image) => image.complete && image.naturalWidth > 0)).catch(() => false), Boolean).catch(() => false);
  const newest = async () => (await api(`${at}/history`)).body?.entries?.[0] ?? null;

  // Ctrl+C, and Ctrl+V with the pointer on the canvas.
  await pick('hei', 'words');
  await page.keyboard.press('ControlOrMeta+c');
  const spot = await empty();
  if (!spot) throw new Error('no empty canvas to paste on');
  await page.mouse.move(spot.x, spot.y);
  await page.keyboard.press('ControlOrMeta+v');
  const pasted = await until('two copies', names, (now) => now.length === 5).catch(async () => names());
  seen.pasted = pasted;
  check('paste_makes_two_with_new_names', pasted.join() === 'hei,hei_2,other,words,words_2', pasted);
  const copyVersions = await versionsOf('hei_2');
  seen.hei_2 = { asset: await asset('hei_2'), ...copyVersions };
  check('same_versions_and_approval', JSON.stringify(copyVersions) === JSON.stringify(heiVersions), { hei: heiVersions, hei_2: copyVersions });
  check('same_tool_its_own_self', seen.hei_2.asset?.tool === cutout.id && seen.hei_2.asset?.fields?.image === '<self.1>', seen.hei_2.asset?.fields);
  const copiedWords = (await asset('words_2'))?.fields?.text;
  check('mention_between_them_names_the_copy', copiedWords === 'a lighthouse in <hei_2.2> beside <other>', copiedWords);
  check('original_words_unchanged', (await asset('words'))?.fields?.text === words, (await asset('words'))?.fields);
  check('no_bytes_copied', bytesHeld() === held0, { before: held0, after: bytesHeld() });
  const laid = await rows();
  seen.rows = laid;
  const same = (a, b) => Math.abs(a) < 2 && Math.abs(b) < 2;
  check('layout_kept', laid.hei_2 && laid.words_2 && same(laid.words_2.x - laid.hei_2.x - (laid.words.x - laid.hei.x), laid.words_2.y - laid.hei_2.y - (laid.words.y - laid.hei.y)), laid);
  const pointer = await onCanvas(spot);
  const middle = (Math.min(laid.hei_2.x, laid.words_2.x) + Math.max(laid.hei_2.x + laid.hei_2.w, laid.words_2.x + laid.words_2.w)) / 2;
  seen.at_pointer = { pointer, middle };
  check('pasted_at_the_pointer', Math.abs(middle - pointer.x) < 40, seen.at_pointer);
  const entry = await newest();
  seen.entry = entry;
  check('one_entry_of_the_history', ['hei_2', 'words_2'].every((name) => entry?.names?.includes(name)), entry);
  check('copy_shows_the_picture', await shows('hei_2'), await boxOf(page, 'hei_2').innerHTML().catch(() => null));
  await shot('pasted');
  // Zoomed to what's selected, both are drawn: a box off the view isn't.
  await quiet();
  await page.keyboard.press('Shift+2');
  await settled(page);
  check('copies_selected', (await selected()).join() === 'hei_2,words_2', await selected());
  await fitCanvas(page);

  // Undo takes both back, at once.
  await quiet();
  await page.keyboard.press('ControlOrMeta+z');
  const undone = await until('both undone', names, (now) => now.length === 3).catch(async () => names());
  check('undo_removes_the_copies', undone.join() === 'hei,other,words', undone);
  await until('gone from the canvas', () => page.locator('.card[data-asset="hei_2"], .card[data-asset="words_2"]').count(), (n) => n === 0).catch(() => null);
  check('undo_clears_the_canvas', (await page.locator('.card[data-asset="hei_2"], .card[data-asset="words_2"]').count()) === 0);

  // With the pointer off the canvas, beside what was copied.
  await page.mouse.move(page.viewportSize().width / 2, 10);
  await quiet();
  await page.keyboard.press('ControlOrMeta+v');
  const beside = await until('two copies again', names, (now) => now.length === 5).catch(async () => names());
  const added = beside.filter((name) => !['hei', 'other', 'words'].includes(name));
  const now = await rows();
  seen.beside = { added, rows: now };
  check('paste_off_the_canvas_lands_beside', added.length === 2 && added.every((name) => now[name] && now[name].x > Math.max(now.hei.x, now.words.x) + 100 && now[name].x < now.other.x + 2000), seen.beside);
  await quiet();
  await page.keyboard.press('ControlOrMeta+z');
  await until('those undone', names, (n) => n.length === 3);

  // Ctrl+D: hei again beside it, its versions with it.
  await pick('hei');
  await page.keyboard.press('ControlOrMeta+d');
  const doubled = await until('a duplicate', names, (n) => n.length === 4).catch(async () => names());
  const twin = doubled.find((name) => /^hei_\d+$/.test(name));
  const twinRows = await rows();
  seen.duplicate = { names: doubled, twin, rows: twinRows };
  check('ctrl_d_duplicates', Boolean(twin) && JSON.stringify(await versionsOf(twin)) === JSON.stringify(heiVersions), seen.duplicate);
  check('ctrl_d_lands_beside', Boolean(twin) && twinRows[twin]?.x > twinRows.hei.x + 100 && Math.abs(twinRows[twin].y - twinRows.hei.y) < 200, seen.duplicate);
  check('ctrl_d_selects_it', (await selected()).join() === twin, await selected());

  // Alt and a drag: a copy lands where it's let go, and what was carried goes back.
  await quiet();
  await page.keyboard.press('Escape');
  const before = (await rows()).other;
  const handle = await boxOf(page, 'other').locator('.nm').first().boundingBox();
  const [x, y] = [handle.x + handle.width / 2, handle.y + handle.height / 2];
  await page.keyboard.down('Alt');
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 2, y + 2);
  await page.mouse.move(x, y + 300, { steps: 6 });
  await page.mouse.up();
  await page.keyboard.up('Alt');
  const dragged = await until('a copy carried out', names, (n) => n.length === 5).catch(async () => names());
  const carried = dragged.find((name) => /^other_\d+$/.test(name));
  const afterDrag = await rows();
  seen.alt_drag = { names: dragged, carried, before, after: afterDrag.other, copy: carried && afterDrag[carried] };
  check('alt_drag_copies', Boolean(carried) && (await asset(carried))?.sha256 === otherFile, seen.alt_drag);
  check('alt_drag_leaves_the_original', Boolean(afterDrag.other) && Math.abs(afterDrag.other.x - before.x) < 2 && Math.abs(afterDrag.other.y - before.y) < 2, seen.alt_drag);
  check('alt_drag_copy_lands_where_let_go', Boolean(carried && afterDrag[carried]) && afterDrag[carried].y > before.y + 150, seen.alt_drag);
  await shot('alt-drag');

  // Duplicate on a box's menu.
  await quiet();
  await boxOf(page, 'words').locator('.nm').first().click({ button: 'right' });
  const menu = page.locator('.asset-menu');
  await menu.waitFor();
  const items = await menu.locator('[role=menuitem]').allTextContents();
  check('menu_has_duplicate', items.includes('Duplicate'), items);
  await menu.getByRole('menuitem', { name: 'Duplicate', exact: true }).click();
  const menued = await until('a duplicate from the menu', names, (n) => n.length === 6).catch(async () => names());
  const again = menued.find((name) => /^words_\d+$/.test(name));
  seen.menu = { names: menued, again };
  check('menu_duplicates', Boolean(again) && (await asset(again))?.fields?.text === words, { ...seen.menu, text: again && (await asset(again))?.fields?.text });

  // Ctrl+V in a box's words: words only, and no box.
  const count = (await names()).length;
  // Opened by its address, as a reload or Back finds it: how a click opens a box is another cell's.
  await page.goto(await projectUrl(id, '/assets/words'));
  await page.locator('.card.here[data-asset="words"]').first().waitFor();
  const field = boxOf(page, 'words').locator('.mention-text[data-field=text]').first();
  await field.waitFor();
  await field.click();
  await page.keyboard.press('End');
  await page.keyboard.press('ControlOrMeta+v');
  const typed = await until('pasted as words', () => field.innerText(), (text) => (text.match(/lighthouse/gi) ?? []).length >= 2, 5000).catch(() => field.innerText());
  seen.words_paste = { typed, names: await names() };
  check('paste_in_words_is_words', (typed.match(/lighthouse/gi) ?? []).length >= 2, seen.words_paste);
  check('paste_in_words_makes_no_box', (await names()).length === count, seen.words_paste);
  await closeBox(page);

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
