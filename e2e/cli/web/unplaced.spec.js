// unplaced (cl-1319). Paul, 11-19: "one thing I've noticed which is a problem with the canvas is that it likes to stack things vertically only,
// so you end up with a big big big long line of nodes all in a column which is kind of dumb. If you have auto-stacking behavior presumably it
// should stack in columns but also use some sort of horizontal stacking as well"; "when you add boxes with no positions, they should probably be
// tiled out both right and left, along a 2D grid"; "most people's screens are 16:9 so the auto-stacking can respect that sort of aspect ratio".
// And: "when you first load a project, you don't need it so zoomed out you see everything all at once, I'd think a max zoomout of like 25% or
// so, with the canvas items mostly centered (fit as many as you can on the screen at once)".
// 30 boxes (pictures, videos, words) made by `cozy apply` with no place, then opened for the first time: laid in rows that wrap, not one column,
// in the file's order, none on another, the whole near 16:9; opened fitted, every box in view, at 25% or more. 400 such boxes: opened at 25%,
// their middle in view. Each opened at 1440×900 and at 1920×1080, kept on paper and dark in web/unplaced (Paul: look at your own UI).
// One box applied alone that reads one there goes right of it, level with it, as a box made from another does (cl-759); a second that reads
// the same box goes under the first.
const { execFile } = require('child_process');
const { appendFileSync, mkdirSync, writeFileSync } = require('fs');
const { randomBytes } = require('crypto');
const { join } = require('path');
const { test, expect, canvasIdle, canvasNames, themed, png, MP4, DARK_OPEN } = require('./fixtures');
const { lying, rowsOf, sameCamera } = require('./geometry');
const { fit } = require('./hands');

const SIZES = [[1440, 900], [1920, 1080]];
const SCREEN = 16 / 9;
const OPEN_ZOOM = 0.25;
const ANIMALS = ['fox', 'owl', 'elk', 'bee', 'cat', 'ant', 'yak', 'emu', 'gnu', 'hen'];
const soft = expect.configure({ soft: true });

/** Box `i` of a file of nodes: a square, wide or tall picture, a video or words, by turns, each picture its own colours. Named so the file's
 *  order is not the names' order. */
function node(i, files) {
  const name = `${ANIMALS[i % ANIMALS.length]}-${Math.floor(i / ANIMALS.length) + 1}`;
  switch (i % 5) {
    case 0: return [name, { kind: 'image', takes: [{ file: files.picture(i % 30, 320, 320) }] }];
    case 1: return [name, { kind: 'video', takes: [{ file: i % 2 ? files.mp4 : files.dark }] }];
    case 2: return [name, { kind: 'image', takes: [{ file: files.picture(i % 30, 360, 240) }] }];
    case 3: return [name, { kind: 'text', text: `Box ${i + 1}: words written into the file of nodes, in its order.` }];
    default: return [name, { kind: 'image', takes: [{ file: files.picture(i % 30, 240, 320) }] }];
  }
}

/** Rows that share a top, in reading order: each a list of names left to right. */
function readingRows(rows, names) {
  const sorted = [...names].sort((a, b) => rows[a].y - rows[b].y || rows[a].x - rows[b].x);
  const lines = [];
  for (const name of sorted) {
    const line = lines.at(-1);
    if (line && Math.abs(rows[line[0]].y - rows[name].y) < 1) line.push(name);
    else lines.push([name]);
  }
  return lines;
}

/** The canvas as it shows: the camera, the area clear of the tools and the top bar, and every box's place on screen. */
const looked = (page) => page.evaluate(() => {
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, pane = root.getBoundingClientRect();
  const clear = { left: Math.max(pane.left, document.querySelector('.tools')?.getBoundingClientRect().right ?? pane.left), top: Math.max(pane.top, document.querySelector('.top')?.getBoundingClientRect().bottom ?? pane.top), right: pane.right, bottom: pane.bottom };
  return { camera: inspection.camera(), clear, cards: Object.fromEntries(inspection.names().map((name) => [name, inspection.card(name).bounds])) };
});

test('unplaced', async ({ context, project, api, signIn, removeAfter }, testInfo) => {
  test.setTimeout(20 * 60_000);
  const shots = join(process.env.OUT, 'web', 'unplaced');
  mkdirSync(shots, { recursive: true });
  const made = new Map();
  const files = {
    mp4: testInfo.outputPath('clip.mp4'),
    dark: testInfo.outputPath('dark.mp4'),
    picture(i, w, h) {
      const path = testInfo.outputPath(`picture-${i}-${w}x${h}.png`);
      if (!made.has(path)) {
        writeFileSync(path, png(0, w, h, false, (x, y) => [(i * 53 + x) & 255, (i * 97 + y) & 255, (i * 29 + ((x + y) >> 2)) & 255]));
        made.set(path, true);
      }
      return path;
    },
  };
  writeFileSync(files.mp4, MP4);
  writeFileSync(files.dark, DARK_OPEN);

  /** A project of its own, as the fixture's. */
  const another = async (label) => {
    const opened = await api.post('/v1/projects', { path: join(process.env.OUT, 'web', 'projects', `web-unplaced-${label}-${randomBytes(3).toString('hex')}`), unencrypted: true });
    removeAfter(opened.path);
    return Object.assign(Object.create(Object.getPrototypeOf(project)), { api, id: opened.id, code: opened.code, path: opened.path, name: opened.name });
  };
  /** `cozy apply -f` in `one`'s folder, as a person's shell runs it there. */
  const apply = (one, nodes, label) => {
    const file = testInfo.outputPath(`${label}.apply.json`);
    writeFileSync(file, JSON.stringify(nodes, null, 2));
    const words = ['--json', 'apply', '-f', file];
    if (process.env.ARGV_LOG) appendFileSync(process.env.ARGV_LOG, `${words.map((word) => `${word}\x1f`).join('')}\x1e`);
    return new Promise((done, failed) => execFile(process.env.COZY, words, { cwd: one.path, maxBuffer: 1 << 26, timeout: 300_000 }, (error, stdout, stderr) => {
      if (error) return failed(new Error(`cozy apply ${label}: exit ${error.code}: ${stderr.slice(-500)}`));
      const said = JSON.parse(stdout);
      said.applied ? done(said) : failed(new Error(`cozy apply ${label}: ${JSON.stringify(said.errors ?? said.conflicts).slice(0, 500)}`));
    }));
  };

  /** `count` boxes applied with no place to `one`, opened for the first time at `width` by `height`: what it laid out and opened at, kept on
   *  paper and on dark (the view it saved restored as it was). */
  const opened = async (one, count, [width, height]) => {
    const label = `${count}-${width}x${height}`;
    const nodes = Object.fromEntries(Array.from({ length: count }, (_, i) => node(i, files)));
    const names = Object.keys(nodes);
    await apply(one, nodes, label);
    await one.look('paper');
    const page = await context.newPage();
    await page.setViewportSize({ width, height });
    await signIn(page, one.url());
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, 'paper');
    const all = (what) => expect.poll(() => canvasNames(page), { message: `${label}: every box on the canvas, ${what}`, timeout: 120_000 }).toHaveLength(count);
    await all('opened');
    const rows = await rowsOf(one, names);
    await canvasIdle(page);
    const seen = await looked(page);
    await expect.poll(async () => { const { viewport } = await api.get(one.at('/view')); return Boolean(viewport) && Math.abs(viewport.zoom - seen.camera.zoom) < 0.0005; }, { message: `${label}: the open's view kept` }).toBe(true);
    const shoot = async (theme) => {
      await soft.poll(() => page.evaluate(() => { const stats = document.querySelector('.canvas-root')?.canvasInspection?.resources(); return stats ? stats.loading + stats.queued : -1; }), { message: `${label} ${theme}: every picture in view loaded`, timeout: 120_000 }).toBe(0);
      await page.screenshot({ path: join(shots, `${count}-boxes-${width}x${height}-${theme}.png`) });
    };
    await shoot('paper');
    await one.look('dark');
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, 'dark');
    await all('reopened');
    await canvasIdle(page);
    const again = (await looked(page)).camera;
    soft(sameCamera(again, seen.camera), `${label}: reopened at the view it kept: ${JSON.stringify(again)}, not ${JSON.stringify(seen.camera)}`).toBe(true);
    await shoot('dark');
    return { one, page, names, rows, seen, label };
  };

  /** Laid out in rows that wrap, in the file's order, the whole near 16:9, none on another. */
  const laidOut = async ({ one, page, names, rows, seen, label }) => {
    const lines = readingRows(rows, names);
    soft(lines.flat(), `${label}: in the file's order, row by row`).toEqual(names);
    soft(lines.length, `${label}: more than one row`).toBeGreaterThan(1);
    soft(Math.max(...lines.map((line) => line.length)), `${label}: not one column`).toBeGreaterThan(2);
    const { zoom } = seen.camera;
    const boxes = names.map((name) => seen.cards[name]);
    const [w, h] = [Math.max(...boxes.map((b) => b.x + b.width)) - Math.min(...boxes.map((b) => b.x)), Math.max(...boxes.map((b) => b.y + b.height)) - Math.min(...boxes.map((b) => b.y))];
    const shape = w / h;
    soft(shape > SCREEN / 1.5 && shape < SCREEN * 1.5, `${label}: the whole near 16:9, ${Math.round(w / zoom)}×${Math.round(h / zoom)}: ${shape.toFixed(2)}`).toBe(true);
    soft(await lying(page, one), `${label}: no box on another`).toEqual([]);
  };

  /** Opened at `OPEN_ZOOM` at the least, the content's middle in the area clear of the tools and the top bar. */
  const openedAt = ({ names, seen, label }) => {
    const { camera, clear, cards } = seen;
    soft(camera.zoom, `${label}: opened at 25% or more`).toBeGreaterThanOrEqual(OPEN_ZOOM - 0.0005);
    const boxes = names.map((name) => cards[name]);
    const middle = { x: (Math.min(...boxes.map((b) => b.x)) + Math.max(...boxes.map((b) => b.x + b.width))) / 2, y: (Math.min(...boxes.map((b) => b.y)) + Math.max(...boxes.map((b) => b.y + b.height))) / 2 };
    soft(middle.x > clear.left && middle.x < clear.right && middle.y > clear.top && middle.y < clear.bottom, `${label}: the content's middle ${JSON.stringify(middle)} in view ${JSON.stringify(clear)}`).toBe(true);
    return boxes;
  };

  for (const [i, size] of SIZES.entries()) {
    await test.step(`30 boxes with no place, applied and opened at ${size.join('×')}: a 16:9 grid in order, all in view at 25% or more`, async () => {
      const one = i === 0 ? project : await another(`30-${size[0]}`);
      const shown = await opened(one, 30, size);
      await laidOut(shown);
      const boxes = openedAt(shown);
      const { clear } = shown.seen;
      const out = shown.names.filter((_, k) => { const b = boxes[k]; return !(b.x >= clear.left - 1 && b.y >= clear.top - 1 && b.x + b.width <= clear.right + 1 && b.y + b.height <= clear.bottom + 1); });
      soft(out, `${shown.label}: every box in view`).toEqual([]);
      await shown.page.close();
    });
  }

  for (const size of SIZES) {
    await test.step(`400 boxes with no place, applied and opened at ${size.join('×')}: at 25%, on their middle`, async () => {
      const shown = await opened(await another(`400-${size[0]}`), 400, size);
      await laidOut(shown);
      openedAt(shown);
      soft(Math.abs(shown.seen.camera.zoom - OPEN_ZOOM), `${shown.label}: too many to fit, so opened at 25%, not further out`).toBeLessThan(0.0005);
      await shown.page.close();
    });
  }

  await test.step('one Edit applied alone that reads owl-1: right of it, level with it; a second: under the first', async () => {
    const one = await another('reads');
    await apply(one, Object.fromEntries([0, 1].map((i) => node(i, files))), 'reads');
    await one.look('paper');
    const page = await context.newPage();
    await page.setViewportSize({ width: 1440, height: 900 });
    await signIn(page, one.url());
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, 'paper');
    await rowsOf(one, ['fox-1', 'owl-1']);
    // Each applied once the page has placed the one before: alone, not a batch.
    for (const name of ['cut-1', 'cut-2']) {
      await apply(one, { [name]: { 'cozy/edit': { timeline: { tracks: [{ clips: [{ clip: '<owl-1>' }] }] } } } }, name);
      await rowsOf(one, [name]);
    }
    await expect.poll(() => canvasNames(page), { message: 'reads: four boxes on the canvas', timeout: 120_000 }).toHaveLength(4);
    await canvasIdle(page);
    const at = await page.evaluate(() => {
      const inspection = document.querySelector('.canvas-root').canvasInspection;
      return { zoom: inspection.camera().zoom, cards: Object.fromEntries(['owl-1', 'cut-1', 'cut-2'].map((name) => { const card = inspection.card(name); return [name, { top: card.bounds.y, bottom: card.bounds.y + card.bounds.height, left: card.body.x, right: card.body.x + card.body.width }]; })) };
    });
    const { zoom, cards: { 'owl-1': owl, 'cut-1': first, 'cut-2': second } } = at;
    const gap = (first.left - owl.right) / zoom;
    soft(gap > 0 && gap <= 120, `reads: cut-1 right of owl-1, ${gap.toFixed(0)} apart: ${JSON.stringify(at)}`).toBe(true);
    soft(Math.abs(first.top - owl.top) / zoom, `reads: cut-1 level with owl-1: ${JSON.stringify(at)}`).toBeLessThan(2);
    soft(Math.abs(second.left - first.left) / zoom, `reads: cut-2 right of owl-1 too: ${JSON.stringify(at)}`).toBeLessThan(2);
    soft(second.top >= first.bottom - 1 && (second.top - first.bottom) / zoom <= 200, `reads: cut-2 just under cut-1: ${JSON.stringify(at)}`).toBe(true);
    soft(await lying(page, one), 'reads: no box on another').toEqual([]);
    await fit(page);
    await page.screenshot({ path: join(shots, 'reads-1440x900-paper.png') });
    await page.close();
  });
});
