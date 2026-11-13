// A click selects a box and a double click opens it; Delete or Backspace deletes what's selected, open or not, with Undo; in words, Backspace is
// only the words' (cl-917, Paul 11-12), in a real browser on a real Calcifer.
//   node select.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, settled, signIn, STEP_MS, until, upload } = require('./page');

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
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-select-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const { id } = await projectFromPage(page);
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${holderToken()}`, 'cozy-project-storage': '3', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).path };
  const asset = async (name) => (await api(at)).assets.find((row) => row.name === name);

  // Two pictures and a text box.
  const pictures = mkdtempSync(join(tmpdir(), 'web-select-'));
  for (const [i, name] of ['first', 'second'].entries()) {
    const path = join(pictures, `${name}.png`);
    writeFileSync(path, png(40 + i * 60, 320, 200));
    await api(`${at}/imports`, 'POST', { source: await upload(holderToken(), path, id), name, exact: true, at: { x: 100 + i * 420, y: 100 } });
  }
  await api(`${at}/assets`, 'POST', { tool: '', kind: 'text', name: 'words', exact: true, fields: { text: 'hello there', node_kind: 'text' }, at: { x: 100, y: 500 } });
  for (const name of ['first', 'second', 'words']) await boxOf(page, name).waitFor();
  await fitCanvas(page);

  const node = (name) => page.locator(`.react-flow__node:has(.card[data-asset="${name}"])`).first();
  const middle = async (name) => {
    const { x, y, width, height } = await node(name).locator('.media').first().boundingBox();
    return { x: x + width / 2, y: y + height / 2 };
  };
  const selected = () => page.locator('.react-flow__node.selected .card').evaluateAll((cards) => cards.map((card) => card.dataset.asset));
  const open = () => page.evaluate(() => ({ focused: document.querySelector('[data-focused-asset]')?.dataset.focusedAsset ?? null, path: location.pathname }));
  const opened = async (name) => {
    const now = await open();
    return now.focused === name && now.path.endsWith(`/assets/${name}`);
  };
  const away = async () => {
    await page.keyboard.press('Escape');
    await page.locator('[data-focused-asset]').waitFor({ state: 'detached' });
    await settled(page);
  };

  // A click selects it, and opens nothing.
  let point = await middle('first');
  await page.mouse.click(point.x, point.y);
  const one = await until('it selected', selected, (now) => now.join() === 'first', 5000).catch(selected);
  // Nothing opened: a box opens on a double click, a moment after the second press; that long gone, none did.
  await page.waitForTimeout(400);
  check('a_click_selects_it', one.join() === 'first', one);
  check('a_click_opens_nothing', !(await open()).focused && !(await open()).path.includes('/assets/'), await open());
  // Another click selects only that one.
  point = await middle('second');
  await page.mouse.click(point.x, point.y);
  const only = await until('only it selected', selected, (now) => now.join() === 'second', 5000).catch(selected);
  check('a_click_selects_only_it', only.join() === 'second', only);
  point = await middle('first');
  await page.mouse.click(point.x, point.y);
  await shot('selected');

  // Backspace deletes it; Undo brings it back.
  await page.keyboard.press('Backspace');
  await until('the box gone', () => node('first').count(), (n) => n === 0);
  await until('the asset gone', () => asset('first'), (row) => !row);
  check('backspace_deletes_the_selected_box', (await node('second').count()) === 1 && Boolean(await asset('second')));
  await page.keyboard.press('ControlOrMeta+z');
  await until('the box back', () => node('first').count(), (n) => n === 1);
  check('undo_brings_it_back', Boolean(await until('the asset back', () => asset('first'), Boolean)));

  // A double click opens it; Backspace there deletes it, and Undo brings it back.
  await settled(page);
  point = await middle('second');
  await page.mouse.dblclick(point.x, point.y);
  await until('second open', () => opened('second'), Boolean);
  await page.locator('[data-focused-asset="second"]:not([data-entering])').waitFor();
  check('a_double_click_opens_it', true);
  await shot('opened');
  await page.keyboard.press('Backspace');
  await until('the open box gone', () => asset('second'), (row) => !row);
  await page.locator('[data-focused-asset]').waitFor({ state: 'detached' });
  check('backspace_deletes_the_open_box', (await node('second').count()) === 0 && !(await open()).path.includes('/assets/'), await open());
  await page.keyboard.press('ControlOrMeta+z');
  await until('the open box back', () => node('second').count(), (n) => n === 1);
  check('undo_brings_the_open_box_back', Boolean(await until('its asset back', () => asset('second'), Boolean)));

  // Delete works as Backspace does.
  await settled(page);
  point = await middle('first');
  await page.mouse.click(point.x, point.y);
  await page.keyboard.press('Delete');
  await until('deleted by Delete', () => asset('first'), (row) => !row);
  check('delete_deletes_the_selected_box', (await node('first').count()) === 0);
  await page.keyboard.press('ControlOrMeta+z');
  await until('back again', () => asset('first'), Boolean);

  // In words, Backspace is the words' alone.
  await settled(page);
  point = await middle('words');
  await page.mouse.dblclick(point.x, point.y);
  await until('words open', () => opened('words'), Boolean);
  const text = page.locator('[data-focused-asset="words"] .mention-text').first();
  await text.click();
  await page.keyboard.press('End');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  const edited = await until('the words edited', async () => (await text.innerText()).trim(), (now) => now === 'hello the', 5000).catch(async () => (await text.innerText()).trim());
  check('backspace_in_words_edits_them', edited === 'hello the', edited);
  check('backspace_in_words_keeps_the_box', (await opened('words')) && Boolean(await asset('words')), await open());
  await away();
  const kept = await until('the words saved', () => asset('words'), (row) => row?.fields?.text === 'hello the').catch(() => null);
  check('words_saved_and_the_box_kept', Boolean(kept), await asset('words'));
  check('every_box_still_there', (await api(at)).assets.map((row) => row.name).sort().join() === 'first,second,words', (await api(at)).assets.map((row) => row.name));

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
