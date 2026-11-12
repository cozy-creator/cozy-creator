// A selected box is obvious on the canvas (cl-926, Paul 11-12): a thick ring of its own colour outside it, with a gap and a glow, at least 3px
// on screen at zoom 0.25 and 1, on paper and dark; several selected all show it; hover and a check's green look otherwise; the box-select
// rectangle shows. In a real browser on a real Calcifer, with screenshots of each.
//   node selected.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { boxOf, calcifer, fitCanvas, lastLook, launch, lookAt, noStall, png, projectFromPage, settled, signIn, STEP_MS, until, upload } = require('./page');

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

const NAMES = ['light', 'dark', 'middle'];

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
  await page.fill('input[aria-label="New project\'s name"]', `web-selected-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const canvas = page.url();
  const { id } = await projectFromPage(page);
  const session = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session') ?? 'null'));
  const api = async (path, method = 'GET', body) => {
    const answer = await fetch(base + path, {
      method,
      headers: { Authorization: `Bearer ${session.token}`, 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const value = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(`${method} ${path}: ${answer.status} ${JSON.stringify(value)}`);
    return value;
  };
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).path };

  // A near-white picture, a near-black one and one between: the ring has to read against each.
  const pictures = mkdtempSync(join(tmpdir(), 'web-selected-'));
  for (const [i, name] of NAMES.entries()) {
    const path = join(pictures, `${name}.png`);
    const grey = [250, 8, 128][i];
    writeFileSync(path, png(0, 320, 200, false, () => [grey, grey, grey]));
    await api(`${at}/imports`, 'POST', { source: await upload(session.token, path), name, exact: true, at: { x: i * 300, y: 0 } });
  }

  const zoomOf = () => page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a);
  const zoomTo = async (wanted) => {
    await page.mouse.move(2, 500);
    await page.keyboard.press('ControlOrMeta+0');
    await settled(page);
    for (let i = 0; i < 12 && (await zoomOf()) > wanted + 0.001; i++) {
      await page.keyboard.press('-');
      await settled(page);
    }
    return zoomOf();
  };
  const middle = async (name) => {
    const { x, y, width, height } = await boxOf(page, name).locator('.media').first().boundingBox();
    return { x: x + width / 2, y: y + height / 2 };
  };
  /** Each box's ring as drawn on screen: its width and offset in screen pixels, its colour, and whether it glows. */
  const rings = () => page.evaluate(() => {
    const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
    const probe = document.createElement('div');
    probe.style.color = 'var(--select)';
    document.body.append(probe);
    const select = getComputedStyle(probe).color;
    probe.style.color = 'var(--cur)';
    const cur = getComputedStyle(probe).color;
    probe.style.color = 'var(--line2)';
    const line = getComputedStyle(probe).color;
    probe.remove();
    const out = { select, cur, line, zoom, boxes: {} };
    for (const node of document.querySelectorAll('.react-flow__node')) {
      const media = node.querySelector('.card > .media');
      if (!media) continue;
      const style = getComputedStyle(media);
      out.boxes[node.querySelector('.card').dataset.asset] = {
        selected: node.classList.contains('selected'),
        style: style.outlineStyle,
        width: parseFloat(style.outlineWidth) * zoom,
        color: style.outlineColor,
        // The glow is the ring's colour, faded.
        glow: style.boxShadow.includes(select.slice(4, -1)),
      };
    }
    return out;
  });
  /** The rings once `ok` holds of them (a selection is a render away), else as they are when it never does, for the check to judge. */
  const ringsWhen = (ok) => until('the rings as wanted', rings, ok, 5000).catch(rings);
  const ringed = (box, look) => box && box.selected && box.style === 'solid' && box.width >= 3 - 0.05 && box.color === look.select && box.glow;
  // Unselected, no ring: an outline-style of none still reports a width.
  const bare = (box, look) => box && !box.selected && !(box.style === 'solid' && box.color === look.select) && !box.glow;

  for (const theme of ['paper', 'dark']) {
    await page.evaluate((theme) => localStorage.setItem('cozy.theme', theme), theme);
    await page.goto(canvas);
    for (const name of NAMES) await boxOf(page, name).waitFor();
    check(`${theme}_theme_shown`, (await page.evaluate(() => document.documentElement.dataset.theme ?? 'paper')) === theme);
    await fitCanvas(page);
    for (const zoom of [1, 0.25]) {
      const tag = `${theme}_${zoom}`;
      seen[`${tag}_zoom`] = await zoomTo(zoom);
      // At the zoom it says: a ring judged at 1 when 0.25 was asked proves nothing of 0.25.
      check(`${tag}_at_its_zoom`, Math.abs(seen[`${tag}_zoom`] - zoom) < 0.01, seen[`${tag}_zoom`]);
      // One, by a click.
      const point = await middle('middle');
      await page.mouse.click(point.x, point.y);
      await page.mouse.move(2, 500);
      const one = (now) => ringed(now.boxes.middle, now) && bare(now.boxes.light, now) && bare(now.boxes.dark, now);
      let look = await ringsWhen(one);
      check(`${tag}_one_ringed`, one(look), look);
      await shot(`${theme}-${zoom}-one`);
      // Several: all of them.
      await page.keyboard.press('ControlOrMeta+a');
      const all = (now) => NAMES.every((name) => ringed(now.boxes[name], now));
      look = await ringsWhen(all);
      check(`${tag}_several_ringed`, all(look), look);
      await shot(`${theme}-${zoom}-several`);
      await page.keyboard.press('Escape');
      await ringsWhen((now) => NAMES.every((name) => !now.boxes[name]?.selected));
    }
    // Hover and a check's green aren't it.
    await zoomTo(1);
    const point = await middle('light');
    await page.mouse.move(point.x, point.y);
    const look = await ringsWhen((now) => now.boxes.light?.style === 'solid');
    check(`${theme}_hover_is_not_selection`, bare(look.boxes.light, look) && look.boxes.light.style === 'solid' && look.boxes.light.width < 3, look.boxes.light);
    check(`${theme}_selection_is_not_the_check_green`, look.select !== look.cur && look.select !== look.line, { select: look.select, cur: look.cur, line: look.line });
    await page.mouse.move(2, 500);

    // The box-select rectangle, while it's drawn: Shift and a drag from open canvas over all three.
    await fitCanvas(page);
    const first = await boxOf(page, 'light').locator('.media').first().boundingBox();
    const last = await boxOf(page, 'middle').locator('.media').first().boundingBox();
    const from = { x: first.x - 40, y: first.y - 60 };
    const to = { x: last.x + last.width + 40, y: last.y + last.height + 60 };
    await page.keyboard.down('Shift');
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
    await page.mouse.move(to.x, to.y, { steps: 4 });
    const rect = await page.evaluate(() => {
      const el = document.querySelector('.react-flow__selection');
      if (!el) return null;
      const style = getComputedStyle(el);
      const probe = document.createElement('div');
      probe.style.color = 'var(--select)';
      document.body.append(probe);
      const select = getComputedStyle(probe).color;
      probe.remove();
      return { width: parseFloat(style.borderTopWidth), color: style.borderTopColor, select, fill: style.backgroundColor };
    });
    await shot(`${theme}-box-select`);
    await page.mouse.up();
    await page.keyboard.up('Shift');
    check(`${theme}_box_select_rectangle_shows`, Boolean(rect) && rect.width >= 1.5 && rect.color === rect.select && rect.fill !== 'rgba(0, 0, 0, 0)', rect);
    const after = await ringsWhen((now) => NAMES.every((name) => ringed(now.boxes[name], now)));
    check(`${theme}_box_select_rings_all`, NAMES.every((name) => ringed(after.boxes[name], after)), after);
    await shot(`${theme}-box-selected`);
    await page.keyboard.press('Escape');
  }
  await page.evaluate(() => localStorage.removeItem('cozy.theme'));

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
