// Zoom from the canvas's corner, as Figma's: − and + step along the page's zooms about the middle of what's visible, the zoom says what it
// is, its menu fits everything, goes to 100%, and works on a box being worked on without leaving it. A box is drawn larger or smaller
// alike, words and all (cl-890). 300 boxes panned at 70% and 11%, with it on the canvas, as see.js measures them.
//   node zoom.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT, $CHROME, $COZY, $SHOT as see.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { projectUrl, importFile, STEP_MS, boxOf, calcifer, closeBox, lastLook, launch, lookAt, noStall, openBox, panning, png, removeGeneratedProject, settled, signIn, summary, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

// The page's own (ZoomControl.tsx's ZOOMS).
const ZOOMS = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 2.5, 3, 4];
const near = (a, b, by = 0.005) => Math.abs(a - b) <= by;

/** The view as the page draws it, and the canvas point at the middle of what's visible: what − and + zoom about. */
const view = (page) => page.evaluate(() => {
  const [, x, y, zoom] = document.querySelector('.react-flow__viewport').style.transform.match(/translate\(([-\d.]+)px, ([-\d.]+)px\) scale\(([\d.]+)\)/).map(Number);
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const left = document.querySelector('.tools')?.getBoundingClientRect().right ?? pane.left;
  const right = document.querySelector('.drawer')?.getBoundingClientRect().left ?? pane.right;
  const [cx, cy] = [(left + right) / 2 - pane.left, pane.height / 2];
  return { zoom, middle: { x: (cx - x) / zoom, y: (cy - y) / zoom }, shown: document.querySelector('.zoom-at')?.textContent };
});
const shows = (at) => at.shown === `${Math.round(at.zoom * 100)}%`;

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const context = await browser.newContext({ viewport: { width: 1800, height: 1200 } });
  const errors = [];
  const watch = (tab) => {
    tab.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
    tab.on('console', (m) => { if (m.type() === 'error' && !/net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
    tab.setDefaultTimeout(STEP_MS);
  };
  const page = await context.newPage();
  watch(page);
  lookAt(page);
  await signIn(page);
  await page.waitForSelector('.top');
  const { token } = await page.evaluate(() => JSON.parse(localStorage.getItem('cozy.session')));
  const api = async (path, init = {}) => (await fetch(base + path, { ...init, headers: { Authorization: `Bearer ${token}`, ...init.headers } })).json();

  const scratch = mkdtempSync(join(tmpdir(), 'web-zoom-'));
  const made = await api('/v1/projects', { method: 'POST', headers: { 'Content-Type': 'application/json', 'Idempotency-Key': `zoom-${Date.now()}` }, body: JSON.stringify({ name: `web-zoom-${Date.now()}`, unencrypted: true }) });
  try {
    // 300 boxes in 18 columns, each a picture (see.js's scale case, with no video).
    writeFileSync(join(scratch, 'a.png'), png(140));
    importFile(made.path, join(scratch, 'a.png'), 'a_picture');
    const held = await api(`/v1/projects/${made.id}`);
    const hash = held.assets.find((row) => row.name === 'a_picture').sha256.replace('sha256:', '').slice(0, 12);
    const COUNT = 300;
    const perRow = Math.ceil(Math.sqrt(COUNT));
    // Its boxes by the assets API as the page makes them, each approved on the picture, whose own box goes; and each one's place by the
    // canvas's PATCH, as the page moves them (cl-803: both are project.db's).
    const cards = {};
    const json = { 'Content-Type': 'application/json' };
    for (let i = 0; i < COUNT; i++) {
      const box = { name: `n${i}`, exact: true, tool: 'see-no-such-package/draw', kind: 'image', fields: { approved: hash, instructions: `box ${i}` } };
      const answer = await api(`/v1/projects/${made.id}/assets`, { method: 'POST', headers: json, body: JSON.stringify(box) });
      if (answer.name !== box.name) throw new Error(`box n${i}: ${JSON.stringify(answer)}`);
      cards[`n${i}`] = { x: (i % perRow) * 304, y: Math.floor(i / perRow) * 240, w: 240 };
    }
    await api(`/v1/projects/${made.id}/assets/a_picture`, { method: 'DELETE' });
    await api(`/v1/projects/${made.id}/canvas`, { method: 'PATCH', headers: json, body: JSON.stringify({ cards }) });
    await until('300 boxes read', async () => (await api(`/v1/projects/${made.id}`)).assets?.length, (n) => n === COUNT);

    const openTab = async (zoom, x) => {
      const tab = await context.newPage();
      watch(tab);
      await tab.addInitScript(([id, zoom, x]) => localStorage.setItem(`cozy.view:${id}`, JSON.stringify({ x, y: 60, zoom })), [made.id, zoom, x]);
      await tab.goto(await projectUrl(made.id));
      await tab.waitForSelector('.react-flow__node');
      await until('every card its thumbnail', () => tab.evaluate(() => {
        const images = [...document.querySelectorAll('.react-flow__node .media img')];
        return images.length >= document.querySelectorAll('.react-flow__node').length && images.every((image) => image.complete);
      }), Boolean);
      await tab.waitForTimeout(500);
      return tab;
    };
    const shot = (tab, name) => (process.env.SHOT ? tab.screenshot({ path: `${process.env.SHOT}-${name}.png` }) : Promise.resolve());

    // Panning with the control on the canvas, as see.js's pan_70 and pan_11: a pan changes no zoom, so the control draws nothing again.
    let tab = await openTab(0.7, 40);
    await tab.evaluate(() => {
      window.zoomChanges = 0;
      new MutationObserver((changes) => (window.zoomChanges += changes.length)).observe(document.querySelector('.zoom'), { subtree: true, childList: true, attributes: true, characterData: true });
    });
    const at70 = await panning(tab, 3000);
    seen.pan_70 = { fewest_cards: at70.fewest, ...summary(at70.frames) };
    check('pan_70_panned', at70.places > 10, at70.places);
    check('pan_leaves_the_control_alone', (await tab.evaluate(() => window.zoomChanges)) === 0, await tab.evaluate(() => window.zoomChanges));
    await tab.close();
    tab = await openTab(0.11, 800);
    const at11 = await panning(tab, 3000);
    seen.pan_11 = { fewest_cards: at11.fewest, ...summary(at11.frames) };
    check('pan_11_shows_all', at11.places > 10 && at11.fewest === COUNT, seen.pan_11);
    await tab.close();

    tab = await openTab(0.7, 40);
    lookAt(tab);
    const start = await view(tab);
    check('zoom_shows_its_percent', start.shown === '70%', start);
    await shot(tab, 'zoom-control');

    // + and −: a step each along the zooms, the middle of what's visible staying where it is.
    const press = async (label) => {
      await tab.click(`.zoom button[aria-label="${label}"]`);
      await settled(tab);
      return view(tab);
    };
    const zoomedIn = await press('Zoom in');
    check('plus_steps', near(zoomedIn.zoom, 0.75) && shows(zoomedIn), zoomedIn);
    check('plus_about_the_middle', near(zoomedIn.middle.x, start.middle.x, 1) && near(zoomedIn.middle.y, start.middle.y, 1), { start: start.middle, after: zoomedIn.middle });
    await press('Zoom out');
    const out = await press('Zoom out');
    check('minus_steps', near(out.zoom, 0.5) && shows(out), out);
    // Two presses at once go two steps, not one and part of the way.
    await tab.click('.zoom button[aria-label="Zoom in"]');
    await tab.click('.zoom button[aria-label="Zoom in"]');
    await settled(tab);
    const twice = await view(tab);
    check('two_presses_two_steps', near(twice.zoom, 0.75) && shows(twice), twice);

    // The zoom opens its menu in place, each with its shortcut.
    const menu = async () => {
      await tab.click('.zoom-at');
      await tab.waitForSelector('.zoom-menu[role=menu]');
    };
    await menu();
    const items = await tab.$$eval('.zoom-menu [role=menuitem]', (rows) => rows.map((row) => [row.querySelector('span').textContent, row.querySelector('kbd')?.textContent ?? null, row.disabled]));
    seen.menu = items;
    const keyOf = Object.fromEntries(items.map(([label, keys]) => [label, keys]));
    check('menu_items', ['Zoom in', 'Zoom out', 'Zoom to fit', 'Zoom to selection', 'Zoom to 50%', 'Zoom to 100%', 'Zoom to 200%'].every((label) => label in keyOf), items);
    // A Mac writes them in its symbols, run together; others in words.
    const shift = (key) => (process.platform === 'darwin' ? `⇧${key}` : `Shift ${key}`);
    check('menu_shortcuts', keyOf['Zoom to fit'] === shift(1) && keyOf['Zoom to selection'] === shift(2) && keyOf['Zoom to 100%'] === shift(0) && /\+$/.test(keyOf['Zoom in']), keyOf);
    check('menu_focused', await tab.evaluate(() => document.activeElement?.getAttribute('role') === 'menuitem'));
    // Once it has opened: it fades in.
    await tab.waitForTimeout(200);
    await shot(tab, 'zoom-menu');
    await tab.click('.zoom-menu >> text=Zoom to 100%');
    await settled(tab);
    const hundred = await view(tab);
    check('menu_100', hundred.zoom === 1 && hundred.shown === '100%' && !(await tab.locator('.zoom-menu').count()), hundred);

    await menu();
    await tab.click('.zoom-menu >> text=Zoom to fit');
    await settled(tab);
    const fitted = await view(tab);
    const inside = await tab.evaluate(() => [...document.querySelectorAll('.react-flow__node')].filter((node) => {
      const box = node.getBoundingClientRect();
      return box.left >= 0 && box.top >= 0 && box.right <= innerWidth && box.bottom <= innerHeight;
    }).length);
    check('menu_fit_all', inside === COUNT && shows(fitted), { inside, ...fitted });
    await tab.keyboard.press('Shift+Digit0');
    await settled(tab);
    check('shift_0_is_100', (await view(tab)).zoom === 1);
    await tab.keyboard.press('Shift+Digit1');
    await settled(tab);
    check('shift_1_fits', near((await view(tab)).zoom, fitted.zoom), await view(tab));

    // A box on the canvas is drawn at its scale, words and all (cl-890): at 50%, 100% and 200% each of its words is the same part of its
    // height, and in and back out it's just as it was. Each part where it is in the box, in canvas pixels.
    const n0 = await boxOf(tab, 'n0').locator('.media').first().boundingBox();
    await tab.keyboard.down('Shift');
    await tab.mouse.click(n0.x + n0.width / 2, n0.y + n0.height / 2);
    await tab.keyboard.up('Shift');
    await tab.keyboard.press('Shift+Digit2');
    await settled(tab);
    const look = () => tab.evaluate(() => {
      const node = [...document.querySelectorAll('.react-flow__node')].find((one) => one.querySelector('.card')?.dataset.asset === 'n0');
      const card = node?.querySelector('.card');
      if (!card) return null;
      const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      const base = card.getBoundingClientRect();
      const pathOf = (el) => {
        const path = [];
        for (let e = el; e && e !== node; e = e.parentElement) path.unshift([...e.parentElement.children].indexOf(e));
        return path.join('.');
      };
      const shown = (el) => el.checkVisibility({ opacityProperty: true, visibilityProperty: true }) && el.getBoundingClientRect().height > 0;
      const own = (el) => [...el.childNodes].some((one) => one.nodeType === 3 && one.textContent.trim());
      const all = [...node.querySelectorAll('*')].filter(shown);
      const inCanvas = (r) => [r.left - base.left, r.top - base.top, r.width, r.height].map((v) => Math.round((v / zoom) * 10) / 10);
      return {
        zoom,
        words: all.filter(own).map((el) => ({ at: pathOf(el), said: el.textContent.trim().slice(0, 24), part: el.getBoundingClientRect().height / base.height, font: getComputedStyle(el).fontSize })),
        parts: all.map((el) => [pathOf(el), ...inCanvas(el.getBoundingClientRect())]),
        style: card.getAttribute('style'),
      };
    });
    const zoomAt = async (label) => {
      await menu();
      await tab.click(`.zoom-menu >> text=${label}`);
      await settled(tab);
      return look();
    };
    const at = { 1: await zoomAt('Zoom to 100%'), 2: await zoomAt('Zoom to 200%'), 0.5: await zoomAt('Zoom to 50%') };
    const again = await zoomAt('Zoom to 100%');
    seen.box_scales = Object.fromEntries(Object.entries(at).map(([zoom, held]) => [zoom, held && { zoom: held.zoom, words: held.words.map(({ said, part }) => [said, Math.round(part * 10000) / 10000]) }]));
    const sameParts = Object.values(at).every((held) => held && held.words.length >= 2 && held.words.length === at[1].words.length
      && held.words.every((word, i) => word.at === at[1].words[i].at && Math.abs(word.part / at[1].words[i].part - 1) < 0.005));
    check('box_words_scale_with_the_canvas', sameParts && near(at[0.5].zoom, 0.5) && at[1].zoom === 1 && near(at[2].zoom, 2), seen.box_scales);
    const drift = again && at[1] && again.parts.length === at[1].parts.length
      ? again.parts.map((part, i) => (part[0] === at[1].parts[i][0] ? Math.max(...part.slice(1).map((v, j) => Math.abs(v - at[1].parts[i][j + 1]))) : Infinity))
      : [Infinity];
    const unchanged = Math.max(...drift) <= 0.5 && again.style === at[1].style && again.words.every((word, i) => word.font === at[1].words[i].font);
    check('box_zoom_in_and_out_leaves_it_as_it_was', unchanged, { drift: Math.max(...drift), before: at[1], after: again });
    await shot(tab, 'zoom-box-scales');
    await tab.keyboard.press('Escape');

    // A box worked on: + draws in further and it stays open; Zoom to fit draws back in on it; Esc in the menu shuts the menu, not the box.
    await openBox(tab, 'n0');
    const here = await view(tab);
    const closer = await press('Zoom in');
    check('here_plus', near(closer.zoom, ZOOMS.find((step) => step > here.zoom * 1.001)) && (await tab.locator('.card.here').count()) === 1, { here, closer });
    await shot(tab, 'zoom-here');
    await menu();
    await tab.keyboard.press('Escape');
    check('here_menu_escape', !(await tab.locator('.zoom-menu').count()) && (await tab.locator('.card.here').count()) === 1);
    await menu();
    await tab.click('.zoom-menu >> text=Zoom to fit');
    await settled(tab);
    const back = await view(tab);
    check('here_fit_is_the_box', near(back.zoom, here.zoom) && (await tab.locator('.card.here').count()) === 1, { here, back });
    await closeBox(tab);
    check('box_left', !(await tab.locator('.card.here').count()) && (await boxOf(tab, 'n0').count()) === 1);
    await tab.close();
  } catch (error) {
    await lastLook();
    throw error;
  } finally {
    await context.close();
    await removeGeneratedProject(made.path, 'web-zoom-');
    rmSync(scratch, { recursive: true, force: true });
  }

  check('no_page_errors', errors.length === 0, errors);
  console.log(JSON.stringify({ failures, seen }, null, 2));
  await browser.close();
  process.exit(failures.length ? 1 : 0);
})().catch(died);

async function died(error) {
  console.error(error);
  await lastLook();
  console.log(JSON.stringify({ failures, seen }, null, 2));
  process.exit(2);
}
