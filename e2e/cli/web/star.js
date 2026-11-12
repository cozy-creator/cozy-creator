// Star an asset and find it again (cl-892, Paul 11-12), in a real browser on a real Calcifer. Star from a box's right click: project.db holds
// `starred` (Calcifer the only writer), nothing goes out of date, and it's no approval; the Files drawer lists it first with a ★; a reload
// keeps it; ControlOrMeta+Z takes it back and Shift brings it again; Unstar returns it to its place.
//   node star.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, calcifer, signIn, STEP_MS, boxOf, closeBox, lastLook, launch, lookAt, noStall, png, rowsOf, settled, until, upload, yamlOf } = require('./page');

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
  const context = await browser.newContext({ viewport: { width: 1600, height: 1100 } });
  const page = await context.newPage();
  lookAt(page);
  page.setDefaultTimeout(STEP_MS);
  const errors = [];
  page.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/status of 4\d\d|net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
  const shot = (name) => (process.env.SHOT ? page.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

  await signIn(page);
  await page.waitForSelector('.top');
  const held = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });
  const post = (path, body) => api(path, { method: 'POST', body: JSON.stringify(body) });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-star-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const folder = (await api(at)).body.path;
  seen.project = { id, folder };
  const canvas = page.url();

  // Three pictures, in this order.
  const pictures = mkdtempSync(join(tmpdir(), 'web-star-'));
  for (const [i, name] of ['first', 'second', 'third'].entries()) {
    const path = join(pictures, `${name}.png`);
    writeFileSync(path, png(30 + i * 40));
    await post(`${at}/imports`, { source: await upload(held?.token, path), name, exact: true, at: { x: 100 + i * 320, y: 100 } });
  }
  const asset = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name);
  const before = await until('three pictures', () => asset('third'), Boolean);

  const files = async () => {
    await page.goto(await projectUrl(id, '/files'));
    const list = page.locator('.drawer .assetlist');
    await list.locator('.assetrow').first().waitFor();
    return list;
  };
  const listed = (list) => list.locator('.assetrow').evaluateAll((rows) => rows.map((row) => `${row.dataset.asset}${row.querySelector('.star') ? '★' : ''}`));
  let list = await files();
  check('files_lists_the_project', (await listed(list)).join() === 'first,second,third', await listed(list));
  check('files_show_pictures', (await list.locator('.asset-thumb img').count()) === 3);
  await shot('files');

  /** A right click on `name`'s box on the canvas, and `item` from its menu. */
  const fromMenu = async (name, item) => {
    if (page.url() !== canvas) await page.goto(canvas);
    await boxOf(page, name).waitFor();
    await page.keyboard.press('f');
    await settled(page);
    await boxOf(page, name).locator('.nm').click({ button: 'right' });
    const menu = page.locator('.asset-menu');
    await menu.waitFor();
    const items = await menu.locator('[role=menuitem]').allTextContents();
    await menu.getByRole('menuitem', { name: item, exact: true }).click();
    return items;
  };
  const starred = () => rowsOf(folder, "SELECT b.name, f.value FROM fields f JOIN boxes b ON b.key = f.box WHERE f.field = 'starred'");

  // Starred: in project.db, the YAML, its history; nothing out of date, and no approval.
  const items = await fromMenu('third', 'Star');
  check('menu_says_star', items.join() === 'Download,Star,Rename,Delete', items);
  const now = await until('starred in its state', () => asset('third'), (row) => row?.starred === true);
  check('project_db_holds_it', starred().length === 1 && starred()[0][0] === 'third' && [true, 'true', 1].includes(starred()[0][1]), starred());
  check('yaml_says_it', /third:\n(?: {4}.*\n)* {4}starred: true/.test(yamlOf(folder)), yamlOf(folder));
  check('nothing_out_of_date', now.standing === before.standing && now.inputs === before.inputs && now.version === before.version, { before, now });
  check('no_approval', now.pinned === before.pinned && now.approved === before.approved, { before, now });
  const history = (await api(`${at}/history`)).body?.entries ?? [];
  check('history_says_starred', history[0]?.words === 'Starred third', history.slice(0, 2).map((entry) => entry.words));

  // It leads Files, with its ★, and a reload keeps it.
  list = await files();
  check('starred_leads_files', (await listed(list)).join() === 'third★,first,second', await listed(list));
  await page.reload();
  list = page.locator('.drawer .assetlist');
  await list.locator('.assetrow').first().waitFor();
  check('reload_keeps_it', (await listed(list)).join() === 'third★,first,second', await listed(list));
  await shot('starred');

  // A click opens it on the canvas.
  await list.locator('.assetrow[data-asset="third"] a').click();
  await page.locator('.card.here[data-asset="third"]').waitFor();
  check('click_opens_it', true);
  await closeBox(page);

  // Undo and Redo, from the page.
  await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  await page.mouse.move(2, 500);
  await page.keyboard.press('ControlOrMeta+z');
  await until('undone', () => asset('third'), (row) => !row?.starred);
  check('undo_unstars', starred().length === 0, starred());
  await page.keyboard.press('ControlOrMeta+Shift+z');
  await until('redone', () => asset('third'), (row) => row?.starred === true);
  check('redo_stars', starred().length === 1, starred());

  // Unstar: back in its place.
  const again = await fromMenu('third', 'Unstar');
  check('menu_says_unstar', again.includes('Unstar') && !again.includes('Star'), again);
  await until('unstarred', () => asset('third'), (row) => !row?.starred);
  list = await files();
  check('unstar_returns_it', (await listed(list)).join() === 'first,second,third', await listed(list));
  check('unstar_leaves_no_field', starred().length === 0 && !/starred/.test(yamlOf(folder)), starred());

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
