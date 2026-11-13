// A box's name as the canvas zooms out (Paul, 11-13: "the text size should be limited ... rather than just truncating"): one line, cut with
// an ellipsis where its box ends, its whole name on hover; never larger on the canvas than the room the layout keeps above a box; and a
// zoom never moves a box. In a real browser on a real Calcifer: pictures and words with short and long names, stacked and side by side as
// close as the layout keeps them (shown.ts underOf and GAP), zoomed by − from 100% to the farthest and back by +, then by the wheel:
// each box where it was, each name one line, none on another box, another's name or its own picture. Pictures at 100%, 45%, 25% and
// the farthest, at 1440×900 and 1920×1080, paper and dark.
//   node labels.js: signed in by page.js's signIn, on the Calcifer whose lock record is $HOLDER
// $PLAYWRIGHT, $CHROME, $SHOT as see.js. Prints what it saw as JSON and exits 1 if anything failed.
const playwright = require(process.env.PLAYWRIGHT);
const { mkdtempSync, rmSync, writeFileSync } = require('fs');
const { tmpdir } = require('os');
const { join } = require('path');
const { addBoxes, calcifer, importFile, lastLook, launch, lookAt, noStall, png, projectFromPage, projectJson, projectUrl, rowsOf, settled, signIn, STEP_MS, until } = require('./page');

const failures = [];
const seen = {};
noStall(seen, died);
function check(name, ok, detail) {
  seen[name] = ok ? true : (detail ?? false);
  if (!ok) failures.push(name);
}

// The page's own (ZoomControl.tsx's ZOOMS): the farthest is the first.
const FARTHEST = 0.05;
// As close as the layout keeps a box under another, under what's drawn of it (shown.ts heightFor and underOf): a picture's row under it,
// its tool and shape line and the rise of the name below; a word box's rise alone; then clear's gap. Side by side, the gap. One more, so
// nothing lies on anything and the page leaves the rows as written.
const UNDER = { image: 40 + 94 + 16 + 1, text: 61 + 16 + 1 };
// The page's body step on screen (tokens.css, at its --ui), which a name keeps down to 25% (Paul, 11-13); and a selected ring's least on screen
// (cl-926), which it keeps down to 15%.
const BODY = 12.5 * 1.35;
const HELD_TO = 0.25;
const RING_LEAST = 3;
const RING_TO = 0.15;
const BESIDE = 240 + 16 + 1;
const COLUMNS = [
  { kind: 'image', names: ['hero_art', 'a_box_whose_name_goes_on_and_on_past_any_width', 'kyogre_art_1', 'x'] },
  { kind: 'text', names: ['notes', 'scene_one_where_the_hero_meets_the_sea', 'mid_name', 'b'] },
  { kind: 'image', names: ['sea', 'background_plate_with_a_long_name', 'c', 'boat_at_dawn'] },
];
const LONG = ['a_box_whose_name_goes_on_and_on_past_any_width', 'scene_one_where_the_hero_meets_the_sea', 'background_plate_with_a_long_name'];
const ALL = COLUMNS.flatMap((column) => column.names);

/** What's drawn, in the page: each box's place on the canvas, its name's lines and size, and what any of its names lies on. */
function audit() {
  const R = (el) => el.getBoundingClientRect();
  const meet = (a, b) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1;
  const shown = (el) => el.checkVisibility?.({ opacityProperty: true, visibilityProperty: true }) && R(el).width >= 1 && R(el).height >= 1;
  const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
  const boxes = [...document.querySelectorAll('.react-flow__node')].map((node) => {
    const at = node.style.transform.match(/translate\(([-\d.]+)px, ?([-\d.]+)px\)/);
    return {
      node,
      name: node.querySelector('.card')?.dataset.asset,
      at: at ? [Number(at[1]), Number(at[2])] : null,
      media: node.querySelector(':scope .card > .media, :scope .card > .tl'),
      labels: [...node.querySelectorAll(':scope .card > .head > *')].filter((el) => !el.matches('.sp') && shown(el)),
      nm: node.querySelector(':scope .card > .head .nm'),
    };
  }).map((box) => {
    // A selected box's ring as drawn: its outline, outside the picture when its offset isn't negative, and nothing drawn over the picture.
    if (!box.node.classList.contains('selected') || !box.media) return box;
    const style = getComputedStyle(box.media);
    const after = getComputedStyle(box.media, '::after').content;
    return { ...box, ring: { width: parseFloat(style.outlineWidth), offset: parseFloat(style.outlineOffset), out: parseFloat(style.outlineWidth) + parseFloat(style.outlineOffset), over: after !== 'none' && after !== 'normal' } };
  });
  const lines = [];
  const on = [];
  const cut = [];
  const sizes = {};
  for (const box of boxes) {
    if (box.nm && shown(box.nm)) {
      const range = document.createRange();
      range.selectNodeContents(box.nm);
      const tops = new Set([...range.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top)));
      if (tops.size > 1) lines.push({ name: box.name, lines: tops.size });
      const style = getComputedStyle(box.nm);
      sizes[box.name] = { canvas: parseFloat(style.fontSize), screen: Math.round(parseFloat(style.fontSize) * zoom * 10) / 10 };
      if (box.nm.scrollWidth > box.nm.clientWidth + 1 && style.textOverflow !== 'ellipsis') cut.push({ name: box.name, why: 'cut with no ellipsis' });
      if (box.nm.title !== box.name) cut.push({ name: box.name, why: `its title is ${JSON.stringify(box.nm.title)}` });
    }
    for (const label of box.labels) {
      const r = R(label);
      const what = `${box.name}'s ${label.className.split(' ')[0]}`;
      if (box.media && shown(box.media) && meet(r, R(box.media))) on.push(`${what} on its picture`);
      // Selected, clear of its ring too, as web-layout's on-picture: the ring is outside the picture, the name over it.
      if (box.ring && r.bottom > R(box.media).top - box.ring.out * zoom + 0.5 && r.right > R(box.media).left && r.left < R(box.media).right) on.push(`${what} on its ring`);
      for (const other of boxes) {
        if (other === box) continue;
        if (meet(r, R(other.node))) on.push(`${what} on ${other.name}`);
        for (const theirs of other.labels) if (meet(r, R(theirs))) on.push(`${what} on ${other.name}'s ${theirs.className.split(' ')[0]}`);
      }
    }
  }
  const rings = boxes.filter((box) => box.ring).map((box) => ({ name: box.name, ...box.ring, on_screen: Math.round(box.ring.width * zoom * 100) / 100 }));
  return { zoom, count: boxes.length, at: Object.fromEntries(boxes.map((box) => [box.name, box.at])), lines, on, cut, sizes, rings };
}

(async () => {
  const base = calcifer();
  const browser = await launch(playwright);
  const errors = [];
  const watch = (tab) => {
    tab.on('pageerror', (error) => errors.push(`pageerror: ${error.message}`));
    tab.on('console', (m) => { if (m.type() === 'error' && !/net::ERR_/.test(m.text())) errors.push(`console: ${m.text()}`); });
    tab.setDefaultTimeout(STEP_MS);
  };
  const contexts = {};
  for (const theme of ['paper', 'dark']) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.addInitScript((dark) => { try { localStorage.setItem('cozy.theme', dark ? 'dark' : 'paper'); } catch {} }, theme === 'dark');
    contexts[theme] = context;
  }
  /** A tab signed in on its own: a session is the tab's. */
  const signedIn = async (theme) => {
    const tab = await contexts[theme].newPage();
    watch(tab);
    await signIn(tab);
    await tab.waitForSelector('.top');
    return tab;
  };
  const paper = await signedIn('paper');
  lookAt(paper);

  const scratch = mkdtempSync(join(tmpdir(), 'web-labels-'));
  // A project as a person makes one, on the page.
  await paper.goto(`${base}/projects`);
  await paper.fill('input[aria-label="New project\'s name"]', `web-labels-${Date.now()}`);
  await paper.getByLabel('Create without encryption', { exact: true }).check();
  await paper.click('form button[type=submit]');
  await paper.waitForURL(/\/p\/[0-9A-HJKMNP-TV-Z]{9}$/);
  const made = await projectFromPage(paper);
  try {
    // Each box apart first: a picture an import (no tool), a word box with none.
    let shade = 30;
    for (const [c, column] of COLUMNS.entries()) {
      for (const [i, name] of column.names.entries()) {
        const at = { x: c * 2000 + i * 500, y: 0 };
        if (column.kind === 'image') {
          const file = join(scratch, `${name}.png`);
          writeFileSync(file, name === 'kyogre_art_1' ? png((shade += 23), 200, 320) : png((shade += 23)));
          importFile(made.path, file, name);
        } else addBoxes(made.path, [{ name, kind: 'text', fields: { text: `The words of ${name}.` }, at }]);
      }
    }
    await until('every box', () => Promise.resolve(projectJson(made.path).assets?.length), (n) => n === ALL.length);

    /** A tab on the project at `zoom`, with `at` (a canvas point) in the middle of what's visible. */
    const open = async (theme, zoom, at, size = { width: 1440, height: 900 }) => {
      const tab = await signedIn(theme);
      await tab.setViewportSize(size);
      // Where it opens: the project's own view (project.db), as see.js and zoom.js set it.
      projectJson(made.id, 'PATCH', '/view', { viewport: { x: Math.round((size.width + 230) / 2 - at.x * zoom), y: Math.round(size.height / 2 - at.y * zoom), zoom } });
      await tab.goto(await projectUrl(made.id));
      await tab.waitForSelector('.react-flow__node');
      await until('every picture drawn', () => tab.evaluate(() => [...document.querySelectorAll('.react-flow__node .media img')].every((image) => image.complete)), Boolean);
      await settled(tab);
      return tab;
    };

    // Each box's height as the page draws it, then all laid out as close as the layout keeps them.
    let tab = await open('paper', 0.1, { x: 4000, y: 0 });
    await until('every box on the page', () => tab.evaluate(() => document.querySelectorAll('.react-flow__node').length), (n) => n === ALL.length);
    const heights = () => tab.evaluate(() => {
      const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      return Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.querySelector('.card')?.dataset.asset, Math.round((node.getBoundingClientRect().height / zoom) * 10) / 10]));
    });
    // Each picture at its own shape (a box is 16:9 until its picture's size arrives, then pushes aside what it lies on): the tall one
    // tall, and nothing changing.
    const tall = await until('every picture its shape', async () => {
      const before = await heights();
      await tab.waitForTimeout(1000);
      const after = await heights();
      return after.kyogre_art_1 > 300 && JSON.stringify(before) === JSON.stringify(after) ? after : null;
    }, Boolean);
    await tab.close();
    const cards = {};
    for (const [c, column] of COLUMNS.entries()) {
      let y = 0;
      for (const name of column.names) {
        cards[name] = { x: c * BESIDE, y: Math.round(y), w: 240 };
        y += Math.ceil(tall[name]) + UNDER[column.kind];
      }
    }
    seen.heights = tall;
    projectJson(made.path, 'PATCH', '/canvas', { cards });
    const rows = () => Object.fromEntries(rowsOf(made.path, 'SELECT name, x, y FROM canvas').map(([name, x, y]) => [name, { x, y }]));
    await until('laid out close', () => Promise.resolve(rows()), (now) => ALL.every((name) => now[name]?.x === cards[name].x && now[name]?.y === cards[name].y));
    // Each column's long name, its second box's, on screen at 100% under the top bar.
    const middle = { x: (3 * BESIDE - 16) / 2, y: 400 };

    // From 100% by − to the farthest and back by +, then out and in by the wheel: where each box is never changes.
    tab = await open('paper', 1, middle);
    lookAt(tab);
    const first = {};
    const looks = [];
    const look = async (how) => {
      await settled(tab);
      await tab.waitForFunction(() => !document.getAnimations().some((held) => held.playState === 'running' && held.effect?.getComputedTiming().iterations !== Infinity), null, { timeout: 5000 }).catch(() => {});
      const now = await tab.evaluate(audit);
      const moved = [];
      for (const [name, at] of Object.entries(now.at)) {
        if (!at) continue;
        first[name] ??= at;
        if (Math.abs(first[name][0] - at[0]) > 0.5 || Math.abs(first[name][1] - at[1]) > 0.5) moved.push(`${name} from ${first[name]} to ${at}`);
      }
      looks.push({ how, zoom: now.zoom, count: now.count, moved, lines: now.lines, on: now.on, cut: now.cut, biggest: Math.max(...Object.values(now.sizes).map((size) => size.canvas)), smallest_on_screen: Math.min(...Object.values(now.sizes).map((size) => size.screen)), rings: now.rings });
      return now;
    };
    const press = async (label) => {
      await tab.click(`.zoom button[aria-label="${label}"]`);
      return look(label);
    };
    let at = await look('100%');
    check('starts_at_100', Math.abs(at.zoom - 1) < 0.005, at.zoom);
    for (let i = 0; i < 20 && at.zoom > FARTHEST + 0.001; i++) at = await press('Zoom out');
    check('reaches_the_farthest', Math.abs(at.zoom - FARTHEST) < 0.005 && at.count === ALL.length, { zoom: at.zoom, count: at.count });
    for (let i = 0; i < 20 && at.zoom < 1 - 0.001; i++) at = await press('Zoom in');
    check('back_at_100', Math.abs(at.zoom - 1) < 0.005, at.zoom);
    // A long name cut short says itself whole under the pointer: its title, on what the pointer finds there.
    const hover = await tab.evaluate((names) => names.map((name) => {
      const nm = document.querySelector(`.react-flow__node .card[data-asset="${name}"] > .head .nm`);
      if (!nm) return { name, found: false };
      const r = nm.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { name, cut: nm.scrollWidth > nm.clientWidth, title: nm.title, under_pointer: hit === nm || nm.contains(hit) };
    }), LONG);
    seen.hover = hover;
    check('a_long_name_is_cut_and_says_itself_under_the_pointer', hover.every((one) => one.found !== false && one.cut && one.title === one.name && one.under_pointer), hover);
    const pane = await tab.locator('.react-flow__pane').boundingBox();
    await tab.mouse.move(pane.x + pane.width / 2, pane.y + pane.height / 2);
    await tab.keyboard.down('Control');
    for (let i = 0; i < 12; i++) await tab.mouse.wheel(0, 120);
    await look('wheel out');
    for (let i = 0; i < 12; i++) await tab.mouse.wheel(0, -120);
    await tab.keyboard.up('Control');
    await look('wheel in');
    seen.looks = looks;
    check('every_box_seen', ALL.every((name) => first[name]), ALL.filter((name) => !first[name]));
    check('a_zoom_moves_no_box', looks.every((one) => !one.moved.length), looks.filter((one) => one.moved.length));
    check('every_name_one_line', looks.every((one) => !one.lines.length), looks.filter((one) => one.lines.length));
    check('no_name_on_anything', looks.every((one) => !one.on.length), looks.filter((one) => one.on.length).map((one) => ({ zoom: one.zoom, on: one.on })));
    check('a_cut_name_ends_in_an_ellipsis_and_says_itself', looks.every((one) => !one.cut.length), looks.filter((one) => one.cut.length));
    // At the body size on screen down to 25%, every name.
    const held = looks.filter((one) => one.zoom >= HELD_TO - 0.001);
    check('names_at_the_body_size_down_to_25', held.length > 5 && held.every((one) => one.smallest_on_screen >= BODY - 0.05), held.map((one) => ({ zoom: one.zoom, smallest: one.smallest_on_screen })));

    // Every box selected, the same way out and back: each ring outside its picture with nothing over the picture, at least 3px on screen
    // down to 15%, and still no name on anything, nor on its own ring; no box moves.
    await tab.mouse.click(pane.x + 4, pane.y + pane.height - 4);
    await tab.keyboard.press('ControlOrMeta+a');
    await until('every box selected', () => tab.evaluate(() => document.querySelectorAll('.react-flow__node.selected').length), (n) => n >= 1);
    const selectedFrom = looks.length;
    at = await look('selected 100%');
    for (let i = 0; i < 20 && at.zoom > FARTHEST + 0.001; i++) at = await press('Zoom out');
    for (let i = 0; i < 20 && at.zoom < 1 - 0.001; i++) at = await press('Zoom in');
    const selectedLooks = looks.slice(selectedFrom);
    seen.selected_looks = selectedLooks.map((one) => ({ zoom: one.zoom, rings: one.rings?.slice(0, 2), on: one.on }));
    check('selected_every_ring_outside_its_picture', selectedLooks.every((one) => one.rings.length && one.rings.every((ring) => ring.offset >= 0 && !ring.over)), selectedLooks.map((one) => ({ zoom: one.zoom, rings: one.rings.filter((ring) => ring.offset < 0 || ring.over) })).filter((one) => one.rings.length));
    check('selected_rings_3px_down_to_15', selectedLooks.filter((one) => one.zoom >= RING_TO - 0.001).every((one) => one.rings.every((ring) => ring.on_screen >= RING_LEAST - 0.05)), selectedLooks.map((one) => ({ zoom: one.zoom, least: Math.min(...one.rings.map((ring) => ring.on_screen)) })));
    check('selected_no_name_on_anything', selectedLooks.every((one) => !one.on.length), selectedLooks.filter((one) => one.on.length).map((one) => ({ zoom: one.zoom, on: one.on })));
    check('selected_a_zoom_moves_no_box', selectedLooks.every((one) => !one.moved.length), selectedLooks.filter((one) => one.moved.length));
    await tab.keyboard.press('Escape');

    await tab.close();

    // Pictures to look at: 100%, 45%, 25% and the farthest, at both sizes, paper and dark, each audited as it's drawn.
    const shots = [];
    for (const size of [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }]) {
      for (const theme of ['paper', 'dark']) {
        for (const zoom of [1, 0.45, 0.25, FARTHEST]) {
          const view = await open(theme, zoom, middle, size);
          const now = await view.evaluate(audit);
          shots.push({ size: `${size.width}x${size.height}`, theme, zoom, lines: now.lines, on: now.on, sizes: Object.values(now.sizes).map((one) => one.screen) });
          if (process.env.SHOT) await view.screenshot({ path: `${process.env.SHOT}-${size.width}x${size.height}-${theme}-${Math.round(zoom * 100)}.png` });
          await view.close();
        }
      }
    }
    seen.shots = shots;
    check('pictured_one_line_on_nothing', shots.every((one) => !one.lines.length && !one.on.length), shots.filter((one) => one.lines.length || one.on.length));
  } catch (error) {
    await lastLook();
    throw error;
  } finally {
    for (const context of Object.values(contexts)) await context.close();
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
