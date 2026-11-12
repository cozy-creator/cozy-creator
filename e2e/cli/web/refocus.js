// Leaving Focus returns the canvas to where it was (cl-931, Paul 11-12: "far too often, when I exit out of focus mode, I am still like zoomed
// way too far in"), in a real browser on a real Calcifer. The canvas panned and zoomed somewhere of its own; a box opened in Focus by a
// double click and left by Esc, its close, Back to canvas and a click off it, its picture zoomed in Focus first, and by way of a second box
// opened from the first: each time the canvas is exactly where it was.
//   node refocus.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER.
// $PLAYWRIGHT: playwright-core's directory; $CHROME: the browser; $SHOT: where to keep its screenshots. Prints what it saw as JSON and exits 1
// if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectFromPage, projectUrl, calcifer, signIn, STEP_MS, boxOf, fitCanvas, lastLook, launch, lookAt, noStall, openCanvas, png, settled, until, upload } = require('./page');

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
  const held = await page.evaluate(() => JSON.parse(sessionStorage.getItem('cozy.session') ?? 'null'));
  const api = (path, init = {}) => page.evaluate(async ({ path, init, token }) => {
    const answer = await fetch(path, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}`, 'cozy-project-storage': '3' } : {}), 'Content-Type': 'application/json' } });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  }, { path, init, token: held?.token });

  await page.goto(`${base}/projects`);
  await page.fill('input[aria-label="New project\'s name"]', `web-refocus-${Date.now()}`);
  await page.getByLabel('Create without encryption', { exact: true }).check();
  await page.click('form button[type=submit]');
  await page.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const id = (await projectFromPage(page)).id;
  const at = `/v1/projects/${id}`;
  seen.project = { id, folder: (await api(at)).body.path };

  // Two pictures far apart.
  const folder = mkdtempSync(join(tmpdir(), 'web-refocus-'));
  for (const [name, shade, x] of [['one', 60, 100], ['two', 190, 1500]]) {
    writeFileSync(join(folder, `${name}.png`), png(shade, 480, 320));
    const made = await api(`${at}/imports`, { method: 'POST', body: JSON.stringify({ source: await upload(held?.token, join(folder, `${name}.png`), id), name, exact: true, at: { x, y: 200 } }) });
    if (made.status >= 300) throw new Error(`${name}: ${JSON.stringify(made)}`);
    await boxOf(page, name).waitFor();
  }
  await fitCanvas(page);
  await settled(page);

  // The canvas somewhere of its own: zoomed out a step and panned.
  await page.click('button[aria-label="Zoom out"]');
  await settled(page);
  const pane = await openCanvas(page);
  await page.mouse.move(pane.x, pane.y);
  await page.mouse.down();
  await page.mouse.move(pane.x - 140, pane.y - 60, { steps: 10 });
  await page.mouse.up();
  await settled(page);

  /** Where the canvas is: its pan and zoom. */
  const viewport = () => page.evaluate(() => {
    const [, x, y, zoom] = document.querySelector('.react-flow__viewport').style.transform.match(/translate\(([-\d.]+)px,\s*([-\d.]+)px\)\s*scale\(([-\d.]+)\)/) ?? [];
    return { x: Number(x), y: Number(y), zoom: Number(zoom) };
  });
  const same = (a, b) => Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5 && Math.abs(a.zoom - b.zoom) < 0.0005;
  const focused = (name) => page.locator(`[data-focused-asset="${name}"]:not([data-entering])`);
  /** Opened as a person opens it: a double click on its picture. */
  const enter = async (name) => {
    const media = await boxOf(page, name).locator('.media').first().boundingBox();
    await page.mouse.dblclick(media.x + media.width / 2, media.y + media.height / 2);
    await focused(name).waitFor();
    await page.waitForTimeout(300);
  };
  const left = async () => {
    await until('Focus left', () => page.locator('.focus-viewport').count(), (n) => n === 0);
    await settled(page);
  };

  const before = await viewport();
  seen.before = before;
  const ways = {
    esc: async () => page.keyboard.press('Escape'),
    close: async () => page.getByRole('button', { name: 'Close focus', exact: true }).click(),
    back: async () => page.getByRole('button', { name: 'Back to canvas', exact: true }).click(),
    click_off: async () => {
      const off = await page.evaluate(() => {
        const own = '.media, .tl, .focus-heading, .run-face, .here-side, .here-under, .image-controls, .focus-image-zoom, button, input, textarea, select, [contenteditable]';
        const room = document.querySelector('.focus-viewport').getBoundingClientRect();
        for (let y = room.bottom - 12; y > room.top + 40; y -= 16) for (let x = room.right - 12; x > room.left + 12; x -= 16) {
          const there = document.elementFromPoint(x, y);
          if (there?.closest('.focus-viewport') && !there.closest(own)) return { x, y };
        }
        throw new Error('no background in Focus');
      });
      await page.mouse.click(off.x, off.y);
    },
  };
  for (const [way, leave] of Object.entries(ways)) {
    await enter('one');
    await leave();
    await left();
    const after = await viewport();
    seen[way] = after;
    check(`left_by_${way}_the_canvas_is_where_it_was`, same(before, after), { before, after });
  }

  // Left at once, while the canvas is still on its way in; by the browser's Back; and in again before it's back, then out.
  const media = async (name) => {
    const box = await boxOf(page, name).locator('.media').first().boundingBox();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  };
  let point = await media('one');
  await page.mouse.dblclick(point.x, point.y);
  await page.waitForTimeout(60);
  await page.keyboard.press('Escape');
  await left();
  seen.at_once = await viewport();
  check('left_at_once_the_canvas_is_where_it_was', same(before, seen.at_once), { before, after: seen.at_once });
  await enter('one');
  await page.goBack();
  await left();
  seen.browser_back = await viewport();
  check('left_by_the_browsers_back_the_canvas_is_where_it_was', same(before, seen.browser_back), { before, after: seen.browser_back });
  await enter('one');
  await page.keyboard.press('Escape');
  await page.waitForTimeout(80);
  point = await media('one');
  await page.mouse.dblclick(point.x, point.y);
  await focused('one').waitFor();
  await page.waitForTimeout(300);
  await page.keyboard.press('Escape');
  await left();
  seen.in_again_on_the_way_back = await viewport();
  check('in_again_on_the_way_back_the_canvas_is_where_it_was', same(before, seen.in_again_on_the_way_back), { before, after: seen.in_again_on_the_way_back });

  // Its picture zoomed in Focus first.
  await enter('one');
  for (let i = 0; i < 3; i++) await page.getByRole('button', { name: 'Zoom in image', exact: true }).click().catch(() => page.keyboard.press('Control+='));
  await page.waitForTimeout(400);
  await page.keyboard.press('Escape');
  await left();
  seen.zoomed_in_focus = await viewport();
  check('zoomed_in_focus_the_canvas_is_where_it_was', same(before, seen.zoomed_in_focus), { before, after: seen.zoomed_in_focus });

  // By way of a second box, opened from the first as a link opens it: back to where it was before the first.
  await enter('one');
  await page.evaluate((path) => {
    history.pushState(null, '', path);
    dispatchEvent(new PopStateEvent('popstate'));
  }, new URL(await projectUrl(id, '/assets/two')).pathname);
  await focused('two').waitFor();
  await page.waitForTimeout(400);
  await shot('second-box');
  await page.keyboard.press('Escape');
  await left();
  seen.by_a_second_box = await viewport();
  check('by_a_second_box_the_canvas_is_where_it_was', same(before, seen.by_a_second_box), { before, after: seen.by_a_second_box });
  await shot('back');

  check('no_page_errors', errors.length === 0, errors);
  await browser.close();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(failures.length ? 1 : 0);
})().catch((why) => died(why.stack ?? why));
