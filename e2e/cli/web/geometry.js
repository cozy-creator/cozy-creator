// Where things are on the canvas, for the canvas cells (cl-973): its camera, a point of the screen on it, the rows the API holds, what's
// drawn on what, and a hand's carry and drag.
const { expect } = require('./fixtures');

/** The camera as the canvas draws it: its pan and zoom. */
const camera = (page) => page.evaluate(() => {
  const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  return { x: m.e, y: m.f, zoom: m.a };
});

/** The same camera, to a pixel. */
const sameCamera = (a, b) => Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5 && Math.abs(a.zoom - b.zoom) < 0.0005;

/** A point of the screen on the canvas, as the page places what's let go there (React Flow's screenToFlowPosition). */
const onCanvas = (page, point) => page.evaluate(({ x, y }) => {
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  return { x: (x - pane.left - m.e) / m.a, y: (y - pane.top - m.f) / m.a };
}, point);

/** Each box's row as the API holds it: `{x, y, w}` by name. */
const rows = async (project) => (await project.api.get(project.at('/canvas'))).cards;

/** Each box drawn, by name: where its node is on the canvas (React Flow's own translate, its row as drawn) and its card's size, both in
 *  canvas units, and the card's rect on the canvas, which a box's body may be narrower than its row and inset in. */
const drawn = (page) => page.evaluate(() => {
  const pane = document.querySelector('.react-flow').getBoundingClientRect();
  const m = new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform);
  return Object.fromEntries([...document.querySelectorAll('.react-flow__node .card[data-asset]')].map((card) => {
    const at = new DOMMatrix(getComputedStyle(card.closest('.react-flow__node')).transform);
    const r = card.getBoundingClientRect();
    const rect = { x: (r.left - pane.left - m.e) / m.a, y: (r.top - pane.top - m.f) / m.a, w: r.width / m.a, h: r.height / m.a };
    return [card.dataset.asset, { x: at.e, y: at.f, w: rect.w, h: rect.h, card: rect }];
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

/** What's drawn in `clip` of the screen, read from a screenshot decoded in the page: its commonest colour, and how many pixels are off it
 *  by more than `by` in a channel, of how many. */
async function colours(page, clip, by = 16) {
  const shot = await page.screenshot({ clip });
  return page.evaluate(async ({ b64, by }) => {
    const bitmap = await createImageBitmap(new Blob([Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))], { type: 'image/png' }));
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext('2d');
    context.drawImage(bitmap, 0, 0);
    const { data } = context.getImageData(0, 0, bitmap.width, bitmap.height);
    const counts = new Map();
    for (let i = 0; i < data.length; i += 4) {
      const key = ((data[i] >> 3) << 10) | ((data[i + 1] >> 3) << 5) | (data[i + 2] >> 3);
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    const common = [...counts].sort((a, b) => b[1] - a[1])[0][0];
    const want = [((common >> 10) & 31) * 8 + 4, ((common >> 5) & 31) * 8 + 4, (common & 31) * 8 + 4];
    let off = 0;
    for (let i = 0; i < data.length; i += 4) if (Math.abs(data[i] - want[0]) > by || Math.abs(data[i + 1] - want[1]) > by || Math.abs(data[i + 2] - want[2]) > by) off++;
    return { common: want, off, of: data.length / 4 };
  }, { b64: shot.toString('base64'), by });
}

module.exports = { camera, carry, colours, drag, drawn, lying, onCanvas, rows, rowsOf, sameCamera };
