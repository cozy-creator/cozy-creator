// glloss (Paul, 11-15: "make sure the new pixijs canvas is robust"): the canvas's WebGL context lost, as a GPU reset, a laptop's sleep or a
// browser reclaiming a background tab loses it, and given back. While it's lost the canvas says so in place and the rest of the page works;
// once it's back every box is drawn again with its picture, with no reload, and a box still moves. Twice over.
const { test, expect, canvasCard, canvasIdle, canvasNames, canvasPoint, openPoint, png, screenshotPixels } = require('./fixtures');
const { carry, rows } = require('./geometry');
const { fit } = require('./hands');

/** A colour per picture, far apart in hue: [r, g, b]. */
const hue = (h) => {
  const f = (n) => { const k = (n + h / 30) % 12; return Math.round(255 * (0.5 - 0.35 * Math.max(-1, Math.min(k - 3, 9 - k, 1)))); };
  return [f(0), f(8), f(4)];
};
const PICTURES = Object.fromEntries(Array.from({ length: 12 }, (_, i) => [`pic_${i + 1}`, hue(i * 30)]));

/** Pictures in view that the screen doesn't show in their own colour: not drawn, or drawn wrong. Ones under the page's own panels are passed. */
async function unpainted(page) {
  const state = await page.evaluate((names) => {
    const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
    if (!inspection) return null;
    const view = root.getBoundingClientRect();
    return { dpr: devicePixelRatio, cards: names.flatMap((name) => {
      const card = inspection.card(name), b = card?.bounds;
      if (!b || b.x + b.width < view.left || b.y + b.height < view.top || b.x > view.right || b.y > view.bottom) return [];
      const body = card.rendered?.body;
      if (!body) return [{ name, drawn: false }];
      const x = body.x + body.width / 2, y = body.y + body.height / 2;
      return root.contains(document.elementFromPoint(x, y)) ? [{ name, drawn: true, x, y }] : [];
    }) };
  }, Object.keys(PICTURES));
  if (!state) return ['no canvas'];
  const read = screenshotPixels(await page.screenshot());
  return state.cards.filter(({ name, drawn, x, y }) => !drawn || !read(x * state.dpr, y * state.dpr).every((value, i) => Math.abs(value - PICTURES[name][i]) <= 12)).map(({ name }) => name);
}

const lost = (page) => page.getByRole('region', { name: 'Project canvas' }).getByRole('alert');

test('glloss', { tag: '@engines' }, async ({ page, project }) => {
  for (const [i, [name, colour]] of Object.entries(PICTURES).entries()) {
    await project.import(png(0, 320, 200, false, () => colour), { name, at: { x: (i % 4) * 300, y: Math.floor(i / 4) * 300 } });
  }
  await project.add({ name: 'notes', kind: 'text', fields: { text: 'Still here after the GPU comes back.' }, at: { x: 1200, y: 0 } });
  await expect.poll(async () => (await canvasNames(page)).length, { message: 'every box on the canvas' }).toBe(13);
  await fit(page);
  const park = await openPoint(page);
  await page.mouse.move(park.x, park.y);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
  await expect.poll(() => unpainted(page), { message: 'every picture drawn in its colour', timeout: 30_000 }).toEqual([]);
  const names = (await canvasNames(page)).sort();
  await page.evaluate(() => { window.__unreloaded = true; });

  for (const cycle of [1, 2]) {
    await test.step(`lost (${cycle}): said in place, and the page goes on`, async () => {
      expect(await page.evaluate(() => {
        const canvas = document.querySelector('.canvas-pixels');
        const gl = canvas?.getContext('webgl2') ?? canvas?.getContext('webgl');
        window.__lose = gl?.getExtension('WEBGL_lose_context');
        window.__lose?.loseContext();
        return Boolean(window.__lose);
      }), 'the browser can lose this context').toBe(true);
      await expect(lost(page), 'the canvas says its graphics are lost').toContainText(/lost/i);
      await expect.soft(page.getByRole('dialog'), 'nothing over the page').toHaveCount(0);
      const menu = page.locator('details.main-menu');
      await menu.locator('> summary').click();
      await expect.soft(menu.locator('nav'), 'the menu opens').toBeVisible();
      await menu.locator('> summary').click();
    });

    await test.step(`restored (${cycle}): every box and picture back, with no reload`, async () => {
      await page.evaluate(() => window.__lose.restoreContext());
      await expect(lost(page), 'the in-place notice goes').toHaveCount(0);
      await canvasIdle(page);
      expect.soft((await canvasNames(page)).sort(), 'the same boxes').toEqual(names);
      await expect.configure({ soft: true }).poll(async () => {
        const missing = [];
        for (const name of [...Object.keys(PICTURES), 'notes']) {
          const shown = (await canvasCard(page, name))?.rendered;
          if (!shown || (name in PICTURES && !shown.media.ready)) missing.push(name);
        }
        return missing;
      }, { message: 'each box drawn, each picture loaded', timeout: 30_000 }).toEqual([]);
      await expect.configure({ soft: true }).poll(() => unpainted(page), { message: 'every picture in its colour on the screen', timeout: 30_000 }).toEqual([]);
      expect.soft(await page.evaluate(() => window.__unreloaded), 'no reload').toBe(true);
    });
  }

  await test.step('a box still moves', async () => {
    const before = (await rows(project)).pic_1;
    const from = await canvasPoint(page, 'pic_1', 'title');
    await carry(page, from, { x: from.x + 120, y: from.y + 60 });
    await expect.poll(async () => {
      const now = (await rows(project)).pic_1;
      return Math.round(now.x - before.x) !== 0 && Math.round(now.y - before.y) !== 0;
    }, { message: 'its move saved' }).toBe(true);
  });
});
