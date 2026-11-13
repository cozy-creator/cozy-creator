// canvas-nav (cl-973 core cell 15), on 300 boxes made by one API batch: − and + step along the zooms about the middle of what's visible; the
// zoom menu's Zoom to fit shows every box and its 100% is scale 1; Shift+1 and Shift+0 do what they do; at 11% a pan by the wheel keeps all 300
// drawn. The dots follow the canvas's scale and are never over a picture. A box's name as the canvas zooms (Paul, 11-13: "the text size
// should be limited ... rather than just truncating"): pictures and words with short and long names, stacked and side by side as close as
// the layout keeps them, zoomed by − from 100% to the farthest and back by +, then by the wheel: each box where it was, each name one line,
// cut with an ellipsis where its box ends and whole under the pointer, on no box, no other name and not its own picture; so too at 100%,
// 45%, 25% and the farthest, at 1440×900 and 1920×1080, paper and dark.
const { test, expect, canvasIdle, openPoint, png } = require('./fixtures');
const { camera, colours, sameCamera } = require('./geometry');

const COUNT = 300;
const ACROSS = 18;
/** The zooms − and + step along (ZoomControl.tsx ZOOMS). */
const ZOOMS = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 2.5, 3, 4];
const stepped = (zoom, by) => (by > 0 ? ZOOMS.find((step) => step > zoom * 1.001) : [...ZOOMS].reverse().find((step) => step < zoom / 1.001));
/** The dots' step on screen at `zoom`: 24 on the canvas, doubled while under 12 (Sheet.tsx Dots). */
const dotStep = (zoom) => {
  let gap = 24 * zoom;
  while (gap < 12) gap *= 2;
  return gap;
};

// The named boxes, above the 300: each column's, top to bottom.
const COLUMNS = [
  { kind: 'image', names: ['hero_art', 'a_box_whose_name_goes_on_and_on_past_any_width', 'kyogre_art_1', 'x'] },
  { kind: 'text', names: ['notes', 'scene_one_where_the_hero_meets_the_sea', 'mid_name', 'b'] },
  { kind: 'image', names: ['sea', 'background_plate_with_a_long_name', 'c', 'boat_at_dawn'] },
];
const LONG = ['a_box_whose_name_goes_on_and_on_past_any_width', 'scene_one_where_the_hero_meets_the_sea', 'background_plate_with_a_long_name'];
const NAMED = COLUMNS.flatMap((column) => column.names);
// As close as the layout keeps a box under another, under what's drawn of it (shown.ts heightFor and underOf): a picture's row under it,
// its tool and shape line and the rise of the name below; a word box's rise alone; then clear's gap. Side by side, the gap. One more, so
// nothing lies on anything and the page leaves the rows as written.
const UNDER = { image: 40 + 55 + 16 + 1, text: 22 + 16 + 1 };
const BESIDE = 240 + 16 + 1;
// Where they're made apart, and where they're laid out close: well clear of the 300, which start at 0,0.
const APART = -6000;
const CLOSE = { x: 0, y: -3000 };

/** What's drawn of the named boxes, in the page: each one's place on the canvas, its name's lines and size, and what any of its names lies
 *  on, of any box. */
function audit(names) {
  const R = (el) => el.getBoundingClientRect();
  const meet = (a, b) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1;
  const shown = (el) => el.checkVisibility?.({ opacityProperty: true, visibilityProperty: true }) && R(el).width >= 1 && R(el).height >= 1;
  const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
  const all = [...document.querySelectorAll('.react-flow__node')].map((node) => {
    const at = node.style.transform.match(/translate\(([-\d.]+)px, ?([-\d.]+)px\)/);
    return {
      node,
      name: node.querySelector('.card')?.dataset.asset,
      at: at ? [Number(at[1]), Number(at[2])] : null,
      media: node.querySelector(':scope .card > .media, :scope .card > .tl'),
      labels: [...node.querySelectorAll(':scope .card > .head > *')].filter((el) => !el.matches('.sp') && shown(el)),
      nm: node.querySelector(':scope .card > .head .nm'),
    };
  });
  const boxes = all.filter((box) => names.includes(box.name));
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
      for (const other of all) {
        if (other === box) continue;
        if (meet(r, R(other.node))) on.push(`${what} on ${other.name}`);
        for (const theirs of other.labels) if (meet(r, R(theirs))) on.push(`${what} on ${other.name}'s ${theirs.className.split(' ')[0]}`);
      }
    }
  }
  return { zoom, count: boxes.length, at: Object.fromEntries(boxes.map((box) => [box.name, box.at])), lines, on, cut, sizes };
}

/** The middle of what's visible, between the tools and the canvas's right edge, as a point of the canvas (CanvasHost.tsx zoomTo). */
const middleOnCanvas = (page) => page.evaluate(() => {
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const left = document.querySelector('.tools')?.getBoundingClientRect().right ?? pane.left;
  const right = document.querySelector('.drawer')?.getBoundingClientRect().left ?? pane.right;
  const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  return { x: ((left + right) / 2 - pane.left - m.e) / m.a, y: (pane.height / 2 - m.f) / m.a };
});

test('canvas-nav', async ({ context, project, api, signIn }) => {
  // One picture of one colour, 300 boxes of it by one apply, and their places by one canvas patch: 18 across, a gap between each.
  const file = await project.upload(png(0, 320, 200, false, () => [190, 40, 40]));
  const nodes = Object.fromEntries(Array.from({ length: COUNT }, (_, i) => [`n${i}`, { kind: 'image', takes: [{ file }] }]));
  const applied = await api.post(project.at('/apply'), { nodes });
  expect(applied.applied, `one apply makes ${COUNT} boxes: ${JSON.stringify(applied.errors ?? []).slice(0, 300)}`).toBe(true);
  const cards = Object.fromEntries(Array.from({ length: COUNT }, (_, i) => [`n${i}`, { x: (i % ACROSS) * 304, y: Math.floor(i / ACROSS) * 240, w: 240 }]));
  await api.patch(project.at('/canvas'), { cards });
  const page = await context.newPage();
  await signIn(page, project.url());
  const cardsDrawn = page.locator('.react-flow__node .card[data-asset]');
  await expect(cardsDrawn.first()).toBeVisible();
  const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
  const zoomMenu = async (item) => {
    await zoomButton.click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  };
  // All of them in view to begin from.
  await zoomMenu('Zoom to fit');
  await expect(cardsDrawn).toHaveCount(COUNT);
  const shows = async (zoom) => expect.soft(zoomButton, `the zoom says ${Math.round(zoom * 100)}%`).toHaveText(`${Math.round(zoom * 100)}%`);
  /** The dots drawn at the step `zoom` asks for. */
  const dotsFollow = async (zoom) => {
    const step = Number(await page.locator('.canvas-dots pattern').getAttribute('width'));
    expect.soft(step, `the dots' step at ${zoom}`).toBeCloseTo(dotStep(zoom), 3);
  };
  /** No dot over the picture of the box nearest the middle, inside its edge. */
  const noDotOnAPicture = async (where) => {
    await page.mouse.move(2, 500);
    const centre = await page.evaluate(() => {
      const pane = document.querySelector('.react-flow').getBoundingClientRect();
      const left = document.querySelector('.tools').getBoundingClientRect().right;
      return { x: (left + pane.right) / 2, y: pane.top + pane.height / 2 };
    });
    const media = await page.evaluate(({ x, y }) => {
      const all = [...document.querySelectorAll('.react-flow__node .card[data-asset] > .media')].map((el) => el.getBoundingClientRect());
      return all.sort((a, b) => Math.hypot(a.x + a.width / 2 - x, a.y + a.height / 2 - y) - Math.hypot(b.x + b.width / 2 - x, b.y + b.height / 2 - y))[0].toJSON();
    }, centre);
    const inset = 6;
    const said = await colours(page, { x: media.x + inset, y: media.y + inset, width: media.width - 2 * inset, height: media.height - 2 * inset });
    expect.soft(said.off, `${where}: no dot over a picture, all ${said.of} pixels its own colour ${said.common}`).toBe(0);
  };

  await test.step('Zoom out and Zoom in', async () => {
    const start = await camera(page);
    for (const [label, by] of [['Zoom out', -1], ['Zoom in', 1]]) {
      const [was, middle] = [(await camera(page)).zoom, await middleOnCanvas(page)];
      await page.getByRole('button', { name: label, exact: true }).click();
      await canvasIdle(page);
      const now = await camera(page);
      expect.soft(now.zoom, `${label} steps from ${was}`).toBeCloseTo(stepped(was, by), 4);
      await shows(now.zoom);
      const after = await middleOnCanvas(page);
      expect.soft(Math.hypot(after.x - middle.x, after.y - middle.y), `${label} keeps the middle where it was`).toBeLessThan(1);
    }
    expect.soft((await camera(page)).zoom, 'out and in again is a step on from the fit').not.toBe(start.zoom);
  });

  let fitted;
  let hundred;
  await test.step("the zoom menu's Zoom to fit and 100%", async () => {
    await zoomMenu('Zoom to fit');
    fitted = await camera(page);
    const inside = await cardsDrawn.evaluateAll((all) => all.filter((card) => {
      const r = card.getBoundingClientRect();
      return r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight;
    }).length);
    expect.soft(inside, 'fit shows every box').toBe(COUNT);
    await shows(fitted.zoom);
    await dotsFollow(fitted.zoom);
    await expect.poll(() => page.locator('.react-flow__node .card[data-asset="n150"] img').evaluate((image) => image.complete && image.naturalWidth > 0), { message: 'the pictures drawn' }).toBe(true);
    await noDotOnAPicture('fitted');

    await zoomMenu('Zoom to 100%');
    hundred = await camera(page);
    expect.soft(hundred.zoom, '100% is scale 1').toBe(1);
    await shows(1);
    await dotsFollow(1);
    const open = await openPoint(page);
    const dots = await colours(page, { x: open.x - 36, y: open.y - 36, width: 72, height: 72 });
    expect.soft(dots.off, 'dots are drawn on open canvas').toBeGreaterThan(0);
    await noDotOnAPicture('at 100%');
  });

  await test.step('Shift+0 and Shift+1', async () => {
    await page.keyboard.press('Shift+Digit1');
    await canvasIdle(page);
    const fit = await camera(page);
    expect.soft(sameCamera(fit, fitted), `Shift+1 is Zoom to fit: ${JSON.stringify(fit)}, ${JSON.stringify(fitted)}`).toBe(true);
    await page.keyboard.press('Shift+Digit0');
    await canvasIdle(page);
    const one = await camera(page);
    expect.soft(sameCamera(one, hundred), `Shift+0 is Zoom to 100%: ${JSON.stringify(one)}, ${JSON.stringify(hundred)}`).toBe(true);
  });

  await test.step('wheel-pan at 11%', async () => {
    // The page's own last view saved first, so the one set here is the one a reload reads.
    const now = await camera(page);
    await expect.poll(async () => {
      const { viewport } = await api.get(project.at('/view'));
      return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
    }, { message: "the page's view saved" }).toBe(true);
    await api.patch(project.at('/view'), { viewport: { x: 800, y: 150, zoom: 0.11 } });
    await page.reload();
    await expect(cardsDrawn).toHaveCount(COUNT);
    await canvasIdle(page);
    expect((await camera(page)).zoom, 'at 11%').toBeCloseTo(0.11, 4);
    await dotsFollow(0.11);
    // Each frame for 2 s, a wheel's sideways pan, 480 px one way then back, as see.js pans: the fewest boxes drawn in any frame.
    const panned = await page.evaluate(() => new Promise((done) => {
      const pane = document.querySelector('.react-flow__pane');
      const viewport = document.querySelector('.react-flow__viewport');
      const places = new Set();
      let [sign, count, fewest] = [1, 0, Infinity];
      const end = performance.now() + 2000;
      const step = (now) => {
        places.add(getComputedStyle(viewport).transform);
        fewest = Math.min(fewest, document.querySelectorAll('.react-flow__node .card[data-asset]').length);
        if (++count % 40 === 0) sign = -sign;
        pane.dispatchEvent(new WheelEvent('wheel', { deltaX: sign * 12, deltaY: 0, clientX: innerWidth / 2, clientY: innerHeight / 2, bubbles: true, cancelable: true }));
        if (now < end) requestAnimationFrame(step);
        else done({ places: places.size, fewest });
      };
      requestAnimationFrame(step);
    }));
    expect.soft(panned.places, 'the wheel pans the canvas').toBeGreaterThan(10);
    expect.soft(panned.fewest, `all ${COUNT} boxes stay drawn while it pans`).toBe(COUNT);
    expect.soft((await camera(page)).zoom, 'a pan changes no zoom').toBeCloseTo(0.11, 4);
  });

  await test.step("names as it zooms: one line, cut where its box ends and whole under the pointer, on nothing; and a zoom moves no box", async () => {
    // Each box apart first: a picture an import (no tool), a word box with none.
    let shade = 30;
    for (const [c, column] of COLUMNS.entries()) {
      for (const [i, name] of column.names.entries()) {
        const at = { x: c * 2000 + i * 500, y: APART };
        if (column.kind === 'image') await project.import(name === 'kyogre_art_1' ? png((shade += 23), 200, 320) : png((shade += 23)), { name, at });
        else await project.add({ name, kind: 'text', fields: { text: `The words of ${name}.` }, at });
      }
    }
    const size = { width: 1440, height: 900 };
    await page.setViewportSize(size);
    /** The page reloaded with `at` (a canvas point) in the middle of what's visible at `zoom`, every picture it draws drawn: its own last
     *  view saved first, so the one set here is the one the reload reads. */
    const viewAt = async (zoom, at) => {
      const now = await camera(page);
      await expect.poll(async () => {
        const { viewport } = await api.get(project.at('/view'));
        return Boolean(viewport) && sameCamera({ x: viewport.x, y: viewport.y, zoom: viewport.zoom }, now);
      }, { message: "the page's view saved" }).toBe(true);
      const { width, height } = page.viewportSize();
      await api.patch(project.at('/view'), { viewport: { x: Math.round((width + 230) / 2 - at.x * zoom), y: Math.round(height / 2 - at.y * zoom), zoom } });
      await page.reload();
      await expect(page.locator('.react-flow__node').first()).toBeVisible();
      await canvasIdle(page);
      await expect.poll(() => page.evaluate(() => [...document.querySelectorAll('.react-flow__node .media img')].every((image) => image.complete)), { message: 'every picture drawn' }).toBe(true);
      expect((await camera(page)).zoom, `at ${zoom}`).toBeCloseTo(zoom, 4);
    };

    // Each box's height as the page draws it, then all laid out as close as the layout keeps them.
    await viewAt(0.1, { x: 4000, y: APART });
    const heights = () => page.evaluate((names) => {
      const zoom = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a;
      return Object.fromEntries([...document.querySelectorAll('.react-flow__node')].map((node) => [node.querySelector('.card')?.dataset.asset, Math.round((node.getBoundingClientRect().height / zoom) * 10) / 10]).filter(([name]) => names.includes(name)));
    }, NAMED);
    // Each picture at its own shape (a box is 16:9 until its picture's size arrives, then pushes aside what it lies on): the tall one tall,
    // and nothing changing a second on.
    let [last, tall] = [null, null];
    await expect.poll(async () => {
      const now = await heights();
      const still = JSON.stringify(now) === JSON.stringify(last);
      last = now;
      if (now.kyogre_art_1 > 300 && Object.keys(now).length === NAMED.length && still) tall = now;
      return Boolean(tall);
    }, { message: 'every picture its shape, and nothing changing', intervals: [1000], timeout: 30_000 }).toBe(true);
    const close = {};
    for (const [c, column] of COLUMNS.entries()) {
      let y = CLOSE.y;
      for (const name of column.names) {
        close[name] = { x: CLOSE.x + c * BESIDE, y: Math.round(y), w: 240 };
        y += Math.ceil(tall[name]) + UNDER[column.kind];
      }
    }
    await api.patch(project.at('/canvas'), { cards: close });
    await expect.poll(async () => {
      const { cards: rows } = await api.get(project.at('/canvas'));
      return NAMED.filter((name) => rows[name]?.x !== close[name].x || rows[name]?.y !== close[name].y);
    }, { message: 'laid out close, and left as written' }).toEqual([]);
    // Each column's long name, its second box's, on screen at 100% under the top bar.
    const middle = { x: CLOSE.x + (3 * BESIDE - 16) / 2, y: CLOSE.y + 400 };

    // From 100% by − to the farthest and back by +, then out and in by the wheel: where each box is never changes.
    await viewAt(1, middle);
    const first = {};
    const looks = [];
    const look = async (how) => {
      await canvasIdle(page);
      await expect.poll(() => page.evaluate(() => !document.getAnimations().some((held) => held.playState === 'running' && held.effect?.getComputedTiming().iterations !== Infinity)), { message: `${how}: nothing animating` }).toBe(true);
      const now = await page.evaluate(audit, NAMED);
      const moved = [];
      for (const [name, at] of Object.entries(now.at)) {
        if (!at) continue;
        first[name] ??= at;
        if (Math.abs(first[name][0] - at[0]) > 0.5 || Math.abs(first[name][1] - at[1]) > 0.5) moved.push(`${name} from ${first[name]} to ${at}`);
      }
      looks.push({ how, zoom: now.zoom, count: now.count, moved, lines: now.lines, on: now.on, cut: now.cut });
      return now;
    };
    const press = async (label) => {
      await page.getByRole('button', { name: label, exact: true }).click();
      return look(label);
    };
    let at = await look('100%');
    expect.soft(at.zoom, 'starts at 100%').toBeCloseTo(1, 2);
    for (let i = 0; i < 20 && at.zoom > ZOOMS[0] + 0.001; i++) at = await press('Zoom out');
    expect.soft({ zoom: Math.round(at.zoom * 1000) / 1000, count: at.count }, 'reaches the farthest, every named box drawn').toEqual({ zoom: ZOOMS[0], count: NAMED.length });
    for (let i = 0; i < 20 && at.zoom < 1 - 0.001; i++) at = await press('Zoom in');
    expect.soft(at.zoom, 'back at 100%').toBeCloseTo(1, 2);
    // A long name cut short says itself whole under the pointer: its title, on what the pointer finds there.
    const hover = await page.evaluate((names) => names.map((name) => {
      const nm = document.querySelector(`.react-flow__node .card[data-asset="${name}"] > .head .nm`);
      if (!nm) return { name, found: false };
      const r = nm.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return { name, cut: nm.scrollWidth > nm.clientWidth, title: nm.title, under_pointer: hit === nm || nm.contains(hit) };
    }), LONG);
    expect.soft(hover, 'a long name is cut, and says itself under the pointer').toEqual(LONG.map((name) => ({ name, cut: true, title: name, under_pointer: true })));
    const pane = await page.locator('.react-flow__pane').boundingBox();
    await page.mouse.move(pane.x + pane.width / 2, pane.y + pane.height / 2);
    await page.keyboard.down('Control');
    for (let i = 0; i < 12; i++) await page.mouse.wheel(0, 120);
    await look('wheel out');
    for (let i = 0; i < 12; i++) await page.mouse.wheel(0, -120);
    await page.keyboard.up('Control');
    await look('wheel in');
    expect.soft(NAMED.filter((name) => !first[name]), 'every named box seen').toEqual([]);
    expect.soft(looks.filter((one) => one.moved.length), 'a zoom moves no box').toEqual([]);
    expect.soft(looks.filter((one) => one.lines.length), 'every name one line').toEqual([]);
    expect.soft(looks.filter((one) => one.on.length).map((one) => ({ zoom: one.zoom, on: one.on })), 'no name on anything').toEqual([]);
    expect.soft(looks.filter((one) => one.cut.length), 'a cut name ends in an ellipsis and says itself').toEqual([]);

    // As each is drawn at 100%, 45%, 25% and the farthest, at both sizes, paper and dark.
    const drawnAt = [];
    for (const each of [size, { width: 1920, height: 1080 }]) {
      await page.setViewportSize(each);
      for (const theme of ['paper', 'dark']) {
        await page.evaluate((theme) => localStorage.setItem('cozy.theme', theme), theme);
        for (const zoom of [1, 0.45, 0.25, ZOOMS[0]]) {
          await viewAt(zoom, middle);
          const now = await page.evaluate(audit, NAMED);
          if (now.lines.length || now.on.length) drawnAt.push({ size: `${each.width}x${each.height}`, theme, zoom, lines: now.lines, on: now.on });
        }
      }
    }
    expect.soft(drawnAt, 'drawn at each zoom, size and theme: each name one line, on nothing').toEqual([]);
  });
});
