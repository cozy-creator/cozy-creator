// A box says its shape as film and media say it (cl-983, Paul 11-13: "I'm seeing 1.75:1 rather than 16:9 (for video). You should be using
// standard aspect ratios, where possible"), in a real browser on a real Calcifer. A take says the shape its render asked for, whatever size
// its model rounded to: project/rounded.py sizes as H3 does, 16:9 at 1344x768, 1.37:1 at 1024x768 (pixels of exactly 4:3), and the latter
// still says 1.37:1 once its box's shape is cleared. A file with no shape asked says its pixels', the nearest standard ratio within about
// 2%: imports of 1344x768, 768x1344 and 1024x1024 say 16:9, 9:16 and 1:1, and 1000x333 the simplest whole ratio, 3:1. A corner pulled near
// 4:3 sets 4:3; pulled to 1.6, far from any, it sets that ratio as it is, and says what it set. Screenshots at 1600 wide, paper and dark.
//   node ratios.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER, on a computer whose own machine runs the tool.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { holderToken, projectFolder, projectFromPage, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, png, settled, until, upload } = require('./page');

const rounded = `${join(__dirname, '..', 'project', 'rounded.py')}/shot`;
const draw = `${join(__dirname, '..', 'project', 'draw.py')}/draw`;
// web/app/src/canvas/ratios.ts's.
const STANDARD = ['16:9', '9:16', '1:1', '4:3', '3:4', '4:5', '5:4', '3:2', '2:3', '21:9', '9:21', '2:1', '1:2', '2.39:1', '1.85:1', '1.90:1', '1.43:1', '1.37:1', '1.66:1'];
const ratioOf = (shape) => shape.split(':').map(Number).reduce((w, h) => w / h);
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
  const held = { token: holderToken() };
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json', ...(init.headers ?? {}) } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s folder"]', projectFolder(`web-ratios-${Date.now()}`));
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  const asset = async (name) => (await api(at)).body?.assets?.find((row) => row.name === name);
  seen.project = { id, folder: (await api(at)).body.path };
  /** What a box at rest says under it of its shape. */
  const said = (name) => boxOf(page, name).locator('.tile-line .ratio-rest').first().innerText().then((text) => text.trim(), () => null);
  const saying = (name, want) => until(`${name} says ${want}`, () => said(name), (text) => text === want).catch(() => said(name));

  // Files with no shape asked: their pixels', snapped.
  const files = mkdtempSync(join(tmpdir(), 'web-ratios-'));
  const imports = [['wide', 1344, 768, '16:9'], ['tall', 768, 1344, '9:16'], ['square', 1024, 1024, '1:1'], ['odd', 1000, 333, '3:1']];
  for (const [i, [name, width, height]] of imports.entries()) {
    writeFileSync(join(files, `${name}.png`), png(60 + 40 * i, width, height));
    const imported = await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(files, `${name}.png`)), name, exact: true, at: { x: 100 + 420 * i, y: 100 } }) });
    if (imported.status >= 300) throw new Error(`import ${name}: ${JSON.stringify(imported)}`);
  }

  // Renders that asked for a shape, sized as H3 sizes them; a box with no take whose corner is pulled.
  const boxes = [
    { name: 'widescreen', tool: rounded, kind: 'video', fields: { instructions: 'a harbour at dawn', aspect_ratio: '16:9' }, at: { x: 100, y: 700 } },
    { name: 'academy', tool: rounded, kind: 'video', fields: { instructions: 'a harbour at dusk', aspect_ratio: '1.37:1' }, at: { x: 560, y: 700 } },
    { name: 'pulled', tool: draw, kind: 'image', fields: { instructions: 'a lighthouse', aspect_ratio: '1:1' }, at: { x: 1020, y: 700 } },
  ];
  for (const box of boxes) {
    const made = await api(`${at}/assets`, { method: 'POST', body: JSON.stringify({ ...box, exact: true }) });
    if (made.status >= 300) throw new Error(`${box.name}: ${JSON.stringify(made)}`);
  }
  await boxOf(page, 'pulled').waitFor();
  await api(`${at}/runs`, { method: 'POST', body: JSON.stringify({ names: ['widescreen', 'academy'], on: ['local'] }), headers: { 'Idempotency-Key': `ratios-${Date.now()}` } });
  const take = (row) => row?.prefixes?.at(-1);
  for (const name of ['widescreen', 'academy']) {
    const landed = await until(`${name}'s take`, () => asset(name), (row) => (take(row)?.main?.width && !take(row).main.absent) || row?.failure, 5 * STEP_MS);
    seen[`${name}_take`] = { failure: landed.failure, aspect_ratio: take(landed)?.aspect_ratio, width: take(landed)?.main?.width, height: take(landed)?.main?.height };
  }
  await fitCanvas(page);
  await settled(page);

  const sizes = { widescreen: [1344, 768, '16:9'], academy: [1024, 768, '1.37:1'] };
  for (const [name, [width, height, asked]] of Object.entries(sizes)) {
    const got = seen[`${name}_take`];
    check(`${name}_made_${width}x${height}_asked_${asked}`, got.width === width && got.height === height && got.aspect_ratio === asked, got);
  }
  for (const [name, , , want] of imports) {
    seen[`${name}_says`] = await saying(name, want);
    check(`${name}_says_${want}`, seen[`${name}_says`] === want, seen[`${name}_says`]);
  }
  seen.widescreen_says = await saying('widescreen', '16:9');
  check('a_16_9_render_of_1344x768_says_16_9', seen.widescreen_says === '16:9', seen.widescreen_says);
  seen.academy_says = await saying('academy', '1.37:1');
  check('a_1_37_render_of_1024x768_says_1_37', seen.academy_says === '1.37:1', seen.academy_says);
  // Its box's shape cleared, its take still says what it asked for, never its pixels' 4:3.
  await api(`${at}/assets/academy`, { method: 'PATCH', body: JSON.stringify({ fields: { aspect_ratio: null } }) });
  await until('academy cleared', () => asset('academy'), (row) => row?.fields?.aspect_ratio == null);
  seen.academy_cleared_says = await saying('academy', '1.37:1');
  check('its_take_says_what_it_asked_once_its_box_is_cleared', seen.academy_cleared_says === '1.37:1', seen.academy_cleared_says);

  /** `pulled`'s corner pulled until its picture is `ratio` wide to 1 tall; what it said held, and what it saved. */
  const pull = async (ratio) => {
    const box = boxOf(page, 'pulled');
    const corner = await box.locator('.corner').first().boundingBox();
    const media = await box.locator('.media').first().boundingBox();
    const before = (await asset('pulled'))?.fields?.aspect_ratio;
    const from = { x: corner.x + corner.width / 2, y: corner.y + corner.height / 2 };
    const by = 80;
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + by, from.y + ((media.width + by) / ratio - media.height), { steps: 20 });
    const live = await page.locator('.ratio-live').first().innerText().then((text) => text.trim(), () => null);
    await page.mouse.up();
    const saved = (await until('its shape saved', () => asset('pulled'), (row) => row?.fields?.aspect_ratio !== before).catch(() => asset('pulled')))?.fields?.aspect_ratio;
    await settled(page);
    return { live, saved, says: await saying('pulled', saved) };
  };
  seen.near_4_3 = await pull(1.32);
  check('pulled_near_4_3_sets_4_3', seen.near_4_3.saved === '4:3' && seen.near_4_3.live === '4:3' && seen.near_4_3.says === '4:3', seen.near_4_3);
  seen.free = await pull(1.6);
  const free = seen.free.saved;
  const freeOk = typeof free === 'string' && !STANDARD.includes(free) && Math.abs(Math.log(ratioOf(free) / 1.6)) < 0.03
    && !STANDARD.some((shape) => Math.abs(Math.log(ratioOf(free) / ratioOf(shape))) <= 0.02);
  check('pulled_far_from_any_stays_as_pulled', freeOk && seen.free.live === free && seen.free.says === free, seen.free);

  await fitCanvas(page);
  await shot('paper');
  await page.evaluate(() => localStorage.setItem('cozy.theme', 'dark'));
  await page.reload();
  await boxOf(page, 'pulled').waitFor();
  await fitCanvas(page);
  const wanted = { wide: '16:9', tall: '9:16', square: '1:1', odd: '3:1', widescreen: '16:9', academy: '1.37:1', pulled: free };
  seen.dark = {};
  for (const [name, want] of Object.entries(wanted)) seen.dark[name] = await saying(name, want);
  check('dark_says_the_same', Object.entries(wanted).every(([name, want]) => seen.dark[name] === want), seen.dark);
  await shot('dark');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
