// Where things are on the canvas, for the canvas cells (cl-973): its camera, a point of the screen on it, the rows the API holds, what's
// drawn on what, and a hand's carry and drag.
const { expect, screenshotPixels } = require('./fixtures');

/** Read-only camera geometry; screenshots prove its presented result separately. */
const camera = (page) => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.camera() ?? null);

/** The same camera, to a pixel. */
const sameCamera = (a, b) => Boolean(a && b) && Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5 && Math.abs(a.zoom - b.zoom) < 0.0005;

/** A screen point in canvas coordinates, accounting for the actual root's origin. */
const onCanvas = (page, point) => page.evaluate(({ x, y }) => {
  const root = document.querySelector('.canvas-root'), view = root.canvasInspection.camera(), pane = root.getBoundingClientRect();
  return { x: (x - pane.left - view.x) / view.zoom, y: (y - pane.top - view.y) / view.zoom };
}, point);

/** Each box's row as the API holds it: `{x, y, w}` by name. */
const rows = async (project) => (await project.api.get(project.at('/canvas'))).cards;

/** Actual retained display transforms, or the committed native card, in world units. Culled cards are absent.
 *  This establishes geometry, not pixel identity. */
const drawn = (page) => page.evaluate(() => {
  const root = document.querySelector('.canvas-root'), inspection = root.canvasInspection, view = inspection.camera(), pane = root.getBoundingClientRect();
  return Object.fromEntries(inspection.names().flatMap(name => {
    const card = inspection.card(name), native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
    if (!card.rendered && !native) return [];
    let rect;
    if (native) {
      const r = native.getBoundingClientRect();
      rect = { x: (r.x - pane.x - view.x) / view.zoom, y: (r.y - pane.y - view.y) / view.zoom, w: r.width / view.zoom, h: r.height / view.zoom };
    } else {
      const transform = card.rendered.transform;
      rect = { x: transform.x, y: transform.y, w: card.bounds.width / view.zoom * transform.scaleX, h: card.bounds.height / view.zoom * transform.scaleY };
    }
    return [[name, { ...rect, card: { ...rect } }]];
  }));
});

const on = (a, b) => a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5;

/** Every pair of boxes on each other, `a on b`: their cards as drawn, and the API's rows at the height each is drawn. [] when none. */
async function lying(page, project) {
  const [shown, held] = [await drawn(page), await rows(project)];
  const names = Object.keys(shown).sort();
  const said = [];
  for (const [i, a] of names.entries()) for (const b of names.slice(i + 1)) {
    if (on(shown[a].card, shown[b].card)) said.push(`${a} on ${b}, drawn`);
    const [ra, rb] = [held[a], held[b]];
    if (ra && rb && on({ ...ra, w: ra.w ?? shown[a].w, h: shown[a].h }, { ...rb, w: rb.w ?? shown[b].w, h: shown[b].h })) said.push(`${a} on ${b}, in the API`);
  }
  return said;
}

/** Carried as a hand carries it: pressed at `from` (a point, or the middle of a locator), leant 12 px across (a carry, not a click or a
 *  scroll), then let go at `to`. `during` looks while it's still held: what it says is what this returns. */
async function carry(page, from, to, during) {
  const at = typeof from.boundingBox === 'function' ? await from.boundingBox() : null;
  const start = at ? { x: at.x + at.width / 2, y: at.y + at.height / 2 } : from;
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + 12 * (Math.sign(to.x - start.x) || 1), start.y, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 10 });
  const said = during ? await during() : undefined;
  await page.mouse.up();
  return said;
}

/** A press at `from` dragged to `to` and let go there, with `modifier` held throughout. */
async function drag(page, from, to, modifier) {
  if (modifier) await page.keyboard.down(modifier);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 4, from.y + 4, { steps: 2 });
  await page.mouse.move(to.x, to.y, { steps: 10 });
  await page.mouse.up();
  if (modifier) await page.keyboard.up(modifier);
}

/** The rows of `names` once the API has them all. */
async function rowsOf(project, names) {
  let held;
  await expect.poll(async () => {
    held = await rows(project);
    return names.filter((name) => !held[name]);
  }, { message: `rows of ${names.join(', ')}` }).toEqual([]);
  return held;
}

/** Screenshot pixels decoded independently of the browser: commonest colour and pixels off it by more than by per channel. */
async function colours(page, clip, by = 16) {
  const read = screenshotPixels(await page.screenshot({ clip })), counts = new Map();
  for (let y = 0; y < read.height; y++) for (let x = 0; x < read.width; x++) {
    const [r, g, b] = read(x, y), key = ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const common = [...counts].sort((a, b) => b[1] - a[1])[0][0];
  const want = [((common >> 10) & 31) * 8 + 4, ((common >> 5) & 31) * 8 + 4, (common & 31) * 8 + 4];
  let off = 0;
  for (let y = 0; y < read.height; y++) for (let x = 0; x < read.width; x++) if (read(x, y).some((v, i) => Math.abs(v - want[i]) > by)) off++;
  return { common: want, off, of: read.width * read.height };
}

module.exports = { camera, carry, colours, drag, drawn, lying, onCanvas, rows, rowsOf, sameCamera };
