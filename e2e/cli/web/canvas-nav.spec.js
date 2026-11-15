// canvas-nav (cl-973 core cell 15), on 300 boxes made by one API batch: − and + step along the zooms about the middle of what's visible; the
// zoom menu's Zoom to fit shows every box and its 100% is scale 1; Shift+1 and Shift+0 do what they do; at 11% a pan by the wheel keeps all 300
// present when in view. The dots follow the canvas's scale and are never over a picture. A box's name as the canvas zooms (Paul, 11-13: "the text size
// should be limited ... rather than just truncating"): pictures and words with short and long names, stacked and side by side as close as
// the layout keeps them, zoomed by − from 100% to the farthest and back by +, then by the wheel: each box where it was, each name one line,
// cut with an ellipsis where its box ends and whole under the pointer, on no box, no other name and not its own picture; so too at 100%,
// 45%, 25% and the farthest, at 1440×900 and 1920×1080, paper and dark.
const { test, expect, canvasNames, canvasIdle, hoverBox, middle: middleOf, openPoint, png, screenshotPixels, pointerOffCanvas } = require('./fixtures');
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
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection;
  const zoom = inspection.camera().zoom;
  const meet = (a, b) => Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x) > 1
    && Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y) > 1;
  const all = inspection.names().flatMap(name => {
    const card = inspection.card(name), painted = card?.rendered;
    if (!painted) return [];
    const labels = ['title', 'version'].flatMap(key => painted.labels[key] ? [{ key, ...painted.labels[key] }] : []);
    const head = [...labels, ...painted.controls.filter(control => control.part === 'delete').map(control => ({ key: 'delete', ...control })),
      ...(painted.stateDot ? [{ key: 'state', ...painted.stateDot }] : [])];
    return [{ name, at: [card.position.x, card.position.y], bounds: card.bounds, media: painted.body, labels: head, nm: painted.labels.title }];
  });
  const boxes = all.filter(box => names.includes(box.name));
  const lines = [], on = [], cut = [], sizes = {}, parts = {}, titles = [];
  for (const box of boxes) {
    if (box.nm) {
      const nm = box.nm;
      if (nm.text.split('\n').length > 1) lines.push({ name: box.name, lines: nm.text.split('\n').length });
      sizes[box.name] = { canvas: nm.fontSize, screen: Math.round(nm.fontSize * zoom * 10) / 10 };
      parts[box.name] = nm.height / box.bounds.height;
      if (nm.text !== nm.fullText && nm.text && !nm.text.endsWith('…')) cut.push({ name: box.name, why: 'cut with no ellipsis' });
      if (nm.x < box.bounds.x - 1 || nm.x + nm.width > box.bounds.x + box.bounds.width + 1) cut.push({ name: box.name, why: 'name outside its box' });
      if (nm.title !== box.name || nm.fullText !== box.name) cut.push({ name: box.name, why: `its title is ${JSON.stringify(nm.title)}` });
      const point = { x: nm.x + nm.width / 2, y: nm.y + nm.height / 2 };
      if (nm.text && nm.fontSize * zoom >= 8 && nm.x >= 0 && nm.y >= 0 && nm.x + nm.width <= innerWidth && nm.y + nm.height <= innerHeight
        && root.contains(document.elementFromPoint(point.x, point.y))) titles.push({ name: box.name, ...nm });
    }
    for (const label of box.labels) {
      const what = `${box.name}'s ${label.key}`;
      if (meet(label, box.media)) on.push(`${what} on its picture`);
      for (const other of all) {
        if (other === box) continue;
        if (meet(label, other.bounds)) on.push(`${what} on ${other.name}`);
        for (const theirs of other.labels) if (meet(label, theirs)) on.push(`${what} on ${other.name}'s ${theirs.key}`);
      }
    }
  }
  const probe = document.createElement('i'); probe.style.color = 'var(--ink)'; document.body.append(probe);
  const css = getComputedStyle(probe).color, values = css.match(/[\d.]+/g).map(Number).slice(0, 3);
  probe.remove();
  return { zoom, count: boxes.length, at: Object.fromEntries(boxes.map(box => [box.name, box.at])), lines, on, cut, sizes, parts, titles,
    ink: css.startsWith('color(') ? values.map(value => value * 255) : values, dpr: devicePixelRatio };
}

async function passive(page) {
  await pointerOffCanvas(page);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.resources()?.scheduledFrame), { message: 'the passive scene is presented and idle' }).toBe(false);
}

/** Geometry reports are accompanied by actual foreground pixels, outside any timing window. */
async function titlePixels(page, look) {
  const read = screenshotPixels(await page.screenshot()), missing = [];
  for (const title of look.titles) {
    let ink = 0;
    for (let y = Math.floor(title.y * look.dpr); y < Math.ceil((title.y + title.height) * look.dpr); y++) {
      for (let x = Math.floor(title.x * look.dpr); x < Math.ceil((title.x + title.width) * look.dpr); x++) {
        if (read(x, y).every((value, i) => Math.abs(value - look.ink[i]) <= 40)) ink++;
      }
    }
    if (!ink) missing.push(title.name);
  }
  return missing;
}

/** All visible fixture photos are actually red in the compositor screenshot; readiness alone cannot pass this. */
async function photosPainted(page, expected) {
  await passive(page);
  let failures = [], count = 0;
  await expect.poll(async () => {
    const state = await page.evaluate(() => {
      const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection;
      const view = root.getBoundingClientRect();
      return { dpr: devicePixelRatio, cards: inspection.names().filter(name => /^n\d+$/.test(name)).flatMap(name => {
        const card = inspection.card(name), body = card?.rendered?.body;
        if (!body) return [];
        const x = body.x + body.width / 2, y = body.y + body.height / 2;
        return x > view.left && x < view.right && y > view.top && y < view.bottom && root.contains(document.elementFromPoint(x, y)) ? [{ name, x, y }] : [];
      }) };
    });
    const read = screenshotPixels(await page.screenshot());
    count = state.cards.length;
    failures = state.cards.filter(({ x, y }) => !read(x * state.dpr, y * state.dpr).every((value, i) => Math.abs(value - [190, 40, 40][i]) <= 12)).map(({ name }) => name);
    return count > 0 && (expected === undefined || count === expected) && !failures.length;
  }, { message: 'every visible fixture photo has its real red pixels', timeout: 30_000 }).toBe(true);
  return { count, failures };
}

/** The middle of the unoccluded viewport, expressed in world coordinates. */
const middleOnCanvas = (page) => page.evaluate(() => {
  const root = document.querySelector('.canvas-root'), pane = root.getBoundingClientRect(), camera = root.canvasInspection.camera();
  const left = document.querySelector('.tools')?.getBoundingClientRect().right ?? pane.left;
  const right = document.querySelector('.drawer')?.getBoundingClientRect().left ?? pane.right;
  return { x: ((left + right) / 2 - pane.left - camera.x) / camera.zoom, y: (pane.height / 2 - camera.y) / camera.zoom };
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
  await expect.poll(() => canvasNames(page)).toHaveLength(COUNT);
  const zoomButton = page.getByRole('button', { name: /more zoom options$/ });
  const zoomMenu = async (item) => {
    await zoomButton.click();
    await page.getByRole('menuitem', { name: new RegExp(`^${item}`) }).click();
    await canvasIdle(page);
  };
  // All of them in view to begin from.
  await zoomMenu('Zoom to fit');
  await expect.poll(() => canvasNames(page)).toHaveLength(COUNT);
  const shows = async (zoom) => expect.soft(zoomButton, `the zoom says ${Math.round(zoom * 100)}%`).toHaveText(`${Math.round(zoom * 100)}%`);
  /** The dots drawn at the step `zoom` asks for. */
  const dotsFollow = async (zoom) => {
    const step = Number(await page.locator('.canvas-dots pattern').getAttribute('width'));
    expect.soft(step, `the dots' step at ${zoom}`).toBeCloseTo(dotStep(zoom), 3);
  };
  /** No dot over the picture of the box nearest the middle, inside its edge. */
  const noDotOnAPicture = async (where) => {
    await passive(page);
    await photosPainted(page);
    const centre = await page.evaluate(() => {
      const pane = document.querySelector('.canvas-root').getBoundingClientRect();
      const left = document.querySelector('.tools').getBoundingClientRect().right;
      return { x: (left + pane.right) / 2, y: pane.top + pane.height / 2 };
    });
    const media = await page.evaluate(({ x, y }) => {
      const inspection = document.querySelector('.canvas-root').canvasInspection;
      const all = inspection.names().map(name => inspection.card(name)?.rendered?.body).filter(Boolean);
      return all.sort((a, b) => Math.hypot(a.x + a.width / 2 - x, a.y + a.height / 2 - y) - Math.hypot(b.x + b.width / 2 - x, b.y + b.height / 2 - y))[0];
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
    await passive(page);
    const inside = await page.evaluate(() => {
      const inspection = document.querySelector('.canvas-root').canvasInspection;
      return inspection.names().filter(name => {
        const card = inspection.card(name), r = card?.bounds;
        return card?.rendered && r.x >= 0 && r.y >= 0 && r.x + r.width <= innerWidth && r.y + r.height <= innerHeight;
      }).length;
    });
    expect.soft(inside, 'fit shows every box').toBe(COUNT);
    await shows(fitted.zoom);
    await dotsFollow(fitted.zoom);
    await photosPainted(page, COUNT);
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
    await expect.poll(() => canvasNames(page)).toHaveLength(COUNT);
    await canvasIdle(page);
    expect((await camera(page)).zoom, 'at 11%').toBeCloseTo(0.11, 4);
    await dotsFollow(0.11);
    await photosPainted(page);
    const point = await openPoint(page);
    await page.mouse.move(point.x, point.y);
    // Sample the same declared 2 s window. Retained offscreen cards are diagnostic; every visible box must remain presented.
    const sampling = page.evaluate(() => new Promise(done => {
      const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, pane = root.getBoundingClientRect();
      const places = new Set(), missing = [];
      let fewestSceneCards = Infinity, fewestNames = Infinity;
      const end = performance.now() + 2000;
      const step = now => {
        places.add(JSON.stringify(inspection.camera()));
        const names = inspection.names();
        let retained = 0;
        for (const name of names) {
          const card = inspection.card(name), bounds = card?.bounds;
          const native = document.querySelector(`.canvas-node .card[data-asset=${JSON.stringify(name)}]`);
          const presented = card?.rendered?.body ?? (native?.checkVisibility() ? native.getBoundingClientRect() : null);
          if (presented) retained++;
          if (bounds && bounds.x + bounds.width > pane.left && bounds.y + bounds.height > pane.top && bounds.x < pane.right && bounds.y < pane.bottom
            && !presented && missing.length < 20) missing.push({ name, at: now });
        }
        fewestNames = Math.min(fewestNames, names.length);
        fewestSceneCards = Math.min(fewestSceneCards, retained);
        if (now < end) requestAnimationFrame(step);
        else done({ places: places.size, fewestNames, fewestSceneCards, missing });
      };
      requestAnimationFrame(step);
    }));
    const end = Date.now() + 2000;
    let count = 0, sign = 1;
    while (Date.now() < end) {
      if (++count % 40 === 0) sign = -sign;
      await page.mouse.wheel(sign * 12, 0);
      await page.evaluate(() => new Promise(done => requestAnimationFrame(done)));
    }
    const panned = await sampling;
    expect.soft(panned.places, 'the wheel pans the canvas').toBeGreaterThan(10);
    expect.soft(panned.fewestNames, `all ${COUNT} boxes stay on the canvas while it pans`).toBe(COUNT);
    expect.soft(panned.missing, `every box in view stays presented while it pans; fewest retained: ${panned.fewestSceneCards}`).toEqual([]);
    await canvasIdle(page);
    await photosPainted(page);
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
      await expect.poll(() => canvasNames(page)).toEqual(expect.arrayContaining(NAMED));
      await canvasIdle(page);
      await passive(page);
      await expect.poll(() => page.evaluate(images => {
        const inspection = document.querySelector('.canvas-root').canvasInspection;
        const painted = images.map(name => inspection.card(name)?.rendered).filter(Boolean);
        return painted.length > 0 && painted.every(card => card.media.ready);
      }, COLUMNS.filter(column => column.kind === 'image').flatMap(column => column.names)), { message: 'every picture in view decoded' }).toBe(true);
      expect((await camera(page)).zoom, `at ${zoom}`).toBeCloseTo(zoom, 4);
    };

    // Each box's height as the page draws it, then all laid out as close as the layout keeps them.
    await viewAt(0.1, { x: 4000, y: APART });
    const heights = () => page.evaluate(names => {
      const inspection = document.querySelector('.canvas-root').canvasInspection, zoom = inspection.camera().zoom;
      return Object.fromEntries(names.flatMap(name => {
        const card = inspection.card(name);
        return card?.rendered ? [[name, Math.round(card.bounds.height / zoom * 10) / 10]] : [];
      }));
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
      // On the clock, that is: a list's edge fades by its scroll (app.css), and runs while the page does.
      await expect.poll(() => page.evaluate(() => document.getAnimations().filter((held) => held.timeline === document.timeline && held.playState === 'running' && held.effect?.getComputedTiming().iterations !== Infinity).map((held) => `${held.animationName ?? held.id} on ${held.effect?.target?.className}`)), { message: `${how}: nothing animating` }).toEqual([]);
      await passive(page);
      await expect.poll(() => page.evaluate(names => { const inspection = document.querySelector('.canvas-root').canvasInspection; return names.filter(name => { const painted = inspection.card(name)?.rendered; return painted?.landing?.active || painted?.glide?.active; }); }, NAMED), { message: `${how}: passive cards at rest` }).toEqual([]);
      const now = await page.evaluate(audit, NAMED);
      expect.soft(await titlePixels(page, now), `${how}: the readable names have actual text pixels`).toEqual([]);
      const moved = [];
      for (const [name, at] of Object.entries(now.at)) {
        if (!at) continue;
        first[name] ??= at;
        if (Math.abs(first[name][0] - at[0]) > 0.5 || Math.abs(first[name][1] - at[1]) > 0.5) moved.push(`${name} from ${first[name]} to ${at}`);
      }
      looks.push({ how, zoom: now.zoom, count: now.count, moved, lines: now.lines, on: now.on, cut: now.cut, sizes: now.sizes, parts: now.parts });
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
    const hover = [];
    for (const name of LONG) {
      const native = await hoverBox(page, name), nm = native.locator('.head .nm');
      const point = await middleOf(nm);
      await page.mouse.move(point.x, point.y);
      hover.push(await nm.evaluate((nm, name) => {
        const r = nm.getBoundingClientRect(), hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return { name, cut: nm.scrollWidth > nm.clientWidth, title: nm.title, under_pointer: hit === nm || nm.contains(hit) };
      }, name));
      await passive(page);
    }
    expect.soft(hover, 'a long name is cut, and says itself under the pointer').toEqual(LONG.map((name) => ({ name, cut: true, title: name, under_pointer: true })));
    const pane = await page.locator('.canvas-root').boundingBox();
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
    // At 50% a name is still the body size on screen (12.5 × 1.35 px; Paul, 11-13: names "at body size out to 25%"), so more of its box
    // than at 100%, never less (cl-885).
    const BODY = 12.5 * 1.35;
    const [full, half] = [looks.find((one) => Math.abs(one.zoom - 1) < 0.005), looks.find((one) => Math.abs(one.zoom - 0.5) < 0.005)];
    // Its part of the box is held against 100% where it was drawn at both: at 100% the view holds only some of the column.
    const small = Object.entries(half?.sizes ?? {}).filter(([name, size]) => !(size.screen >= BODY - 0.05 && (full.parts[name] === undefined || half.parts[name] >= full.parts[name] - 1e-6))).map(([name, size]) => ({ name, screen: size.screen, part: half.parts[name], at_100: full.parts[name] }));
    expect.soft(half && Object.keys(half.sizes).length > 0 ? small : 'no look at 50%', 'at 50% every name the body size on screen, more of its box than at 100%').toEqual([]);

    // As each is drawn at 100%, 45%, 25% and the farthest, at both sizes, paper and dark.
    const drawnAt = [];
    for (const each of [size, { width: 1920, height: 1080 }]) {
      await page.setViewportSize(each);
      for (const theme of ['paper', 'dark']) {
        await page.evaluate((theme) => localStorage.setItem('cozy.theme', theme), theme);
        for (const zoom of [1, 0.45, 0.25, ZOOMS[0]]) {
          await viewAt(zoom, middle);
          const now = await page.evaluate(audit, NAMED);
          expect.soft(await titlePixels(page, now), `${theme} at ${zoom}: readable names have text pixels`).toEqual([]);
          if (now.lines.length || now.on.length) drawnAt.push({ size: `${each.width}x${each.height}`, theme, zoom, lines: now.lines, on: now.on });
        }
      }
    }
    expect.soft(drawnAt, 'drawn at each zoom, size and theme: each name one line, on nothing').toEqual([]);
  });
});
