// isolation (Paul, 11-15: "make sure the new pixijs canvas is robust"; B1, cl-1013, showed one card's throw dropping the whole scene's
// update): one box the canvas can't draw takes nothing else with it. A box whose drawing throws (its name, when measured, throws: the way a
// card's own bug does) is skipped or drawn broken and said at most once, and every other box goes on being drawn as the canvas zooms and
// as boxes arrive after it. Boxes of odd but valid data (200 mentions, missing ones among them; 20,000 characters; emoji and right-to-left
// words; a take whose file is gone from disk) are drawn with no page error, the gone file as a placeholder asked for a few times, not forever.
const { createHash } = require('crypto');
const { readdirSync, rmSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasCard, canvasIdle, canvasNames, openPoint, png, screenshotPixels } = require('./fixtures');
const { fit } = require('./hands');

const MARKER = 'zz_broken_marker';
const THROWN = `injected: ${MARKER} cannot be measured`;
const hue = (h) => {
  const f = (n) => { const k = (n + h / 30) % 12; return Math.round(255 * (0.5 - 0.35 * Math.max(-1, Math.min(k - 3, 9 - k, 1)))); };
  return [f(0), f(8), f(4)];
};
const PICTURES = Object.fromEntries(Array.from({ length: 15 }, (_, i) => [`pic_${i + 1}`, hue(i * 24)]));
const WORDS = ['notes', 'outline', 'credits'];

/** Boxes in view that aren't drawn, and pictures the screen doesn't show in their own colour. Ones under the page's own panels are passed. */
async function undrawn(page, names) {
  const state = await page.evaluate((names) => {
    const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
    if (!inspection) return null;
    const view = root.getBoundingClientRect();
    return { dpr: devicePixelRatio, cards: names.flatMap((name) => {
      const card = inspection.card(name), b = card?.bounds;
      if (!b) return [{ name, drawn: false }];
      if (b.x + b.width < view.left || b.y + b.height < view.top || b.x > view.right || b.y > view.bottom) return [];
      const body = card.rendered?.body;
      if (!body) return [{ name, drawn: false }];
      const x = body.x + body.width / 2, y = body.y + body.height / 2;
      return root.contains(document.elementFromPoint(x, y)) ? [{ name, drawn: true, x, y }] : [];
    }) };
  }, names);
  if (!state) return ['no canvas'];
  const read = screenshotPixels(await page.screenshot());
  return state.cards.filter(({ name, drawn, x, y }) => !drawn || (PICTURES[name] && !read(x * state.dpr, y * state.dpr).every((value, i) => Math.abs(value - PICTURES[name][i]) <= 12))).map(({ name }) => name);
}

async function parked(page) {
  const { x, y } = await openPoint(page);
  await page.mouse.move(x, y);
  await expect(page.locator('.canvas-node')).toHaveCount(0);
}

test('isolation', async ({ page, project, guards }) => {
  // What names the broken box is counted here, once at most, not by the guard.
  guards.allow(new RegExp(MARKER));
  const said = [];
  page.on('pageerror', (error) => { if (error.message.includes(MARKER)) said.push(`page error: ${error.message}`); });
  page.on('console', (message) => { if (message.type() === 'error' && message.text().includes(MARKER)) said.push(`console error: ${message.text()}`); });
  for (const [i, [name, colour]] of Object.entries(PICTURES).entries()) {
    await project.import(png(0, 320, 200, false, () => colour), { name, at: { x: (i % 5) * 300, y: Math.floor(i / 5) * 300 } });
  }
  for (const [i, name] of WORDS.entries()) await project.add({ name, kind: 'text', fields: { text: `The ${name}, in words.` }, at: { x: i * 300, y: 900 } });
  const others = [...Object.keys(PICTURES), ...WORDS];
  await expect.poll(async () => (await canvasNames(page)).length, { message: 'every box on the canvas' }).toBe(others.length);
  await fit(page);
  await parked(page);
  await expect.poll(() => undrawn(page, others), { message: 'every box drawn, each picture in its colour', timeout: 30_000 }).toEqual([]);

  await test.step('a box whose drawing throws arrives: the rest drawn, as the canvas zooms and as boxes come after it', async () => {
    // A card's own bug, made on purpose: any text holding its name throws when measured, as Scene's title fit and Pixi's text both do.
    await page.evaluate(({ marker, thrown }) => {
      for (const Context of [globalThis.CanvasRenderingContext2D, globalThis.OffscreenCanvasRenderingContext2D].filter(Boolean)) {
        const measure = Context.prototype.measureText;
        Context.prototype.measureText = function (text) {
          if (String(text).includes(marker)) throw new Error(thrown);
          return measure.call(this, text);
        };
      }
    }, { marker: MARKER, thrown: THROWN });
    await project.add({ name: MARKER, kind: 'text', fields: { text: 'One box the canvas cannot draw.' }, at: { x: 300, y: 1200 } });
    await project.add({ name: 'after', kind: 'text', fields: { text: 'Made after the broken one.' }, at: { x: 600, y: 1200 } });
    await expect.poll(async () => (await canvasNames(page)).includes(MARKER) && (await canvasNames(page)).includes('after'), { message: 'both on the canvas' }).toBe(true);
    await fit(page);
    await parked(page);
    const all = [...others, 'after'];
    await expect.configure({ soft: true }).poll(() => undrawn(page, all), { message: 'every other box drawn, the one after it too', timeout: 30_000 }).toEqual([]);
    for (const [button, way] of [['Zoom out', 'out'], ['Zoom in', 'back in']]) {
      await page.getByRole('button', { name: button, exact: true }).click();
      await canvasIdle(page);
      await parked(page);
      await expect.configure({ soft: true }).poll(() => undrawn(page, all), { message: `zoomed ${way}: every other box drawn`, timeout: 30_000 }).toEqual([]);
    }
    await project.add({ name: 'later', kind: 'text', fields: { text: 'Made once the canvas had zoomed.' }, at: { x: 900, y: 1200 } });
    await expect.configure({ soft: true }).poll(async () => Boolean((await canvasCard(page, 'later'))?.rendered), { message: 'a box made later drawn' }).toBe(true);
    expect.soft(said.length, `said at most once: ${said.slice(0, 3).join(' | ')}${said.length > 3 ? ` … ${said.length} in all` : ''}`).toBeLessThanOrEqual(1);
  });

  await test.step('odd but valid boxes, and a take whose file is gone: drawn, no page error, the gone file asked for a few times only', async () => {
    const mentions = [...Array.from({ length: 180 }, (_, i) => `<pic_${(i % 15) + 1}>`), ...Array.from({ length: 20 }, (_, i) => `<nobody_${i}>`)].join(' ');
    const odd = {
      many: `A crowd: ${mentions}.`,
      long: 'A very long prompt. '.repeat(1000),
      scripts: 'مرحبا بالعالم 👋🏽 👨‍👩‍👧‍👦 שלום עולם 🏳️‍🌈 नमस्ते 漢字かなカナ',
    };
    for (const [i, [name, text]] of Object.entries(odd).entries()) await project.add({ name, kind: 'text', fields: { text }, at: { x: i * 300, y: 1500 } });
    const gone = png(0, 320, 200, false, () => [90, 90, 90]);
    await project.import(gone, { name: 'gone_file', at: { x: 900, y: 1500 } });
    const sha = createHash('sha256').update(gone).digest('hex');
    const files = readdirSync(join(project.path, 'files')).filter((file) => file.startsWith(sha));
    expect(files.length, 'its file in the project').toBeGreaterThan(0);
    for (const file of files) rmSync(join(project.path, 'files', file));
    // Its file is gone on purpose: each ask's 404 is the browser's own line. How often it asks is judged below.
    guards.allow(new RegExp(`status of 404\\b.*${sha}`));
    // A fresh page, so nothing of the gone file is held by the tab.
    const asks = [];
    page.on('request', (request) => { if (request.url().includes(sha)) asks.push(Date.now()); });
    await page.reload();
    await expect.poll(async () => (await canvasNames(page)).length, { message: 'every box on the canvas after the reload' }).toBeGreaterThanOrEqual(others.length + 7);
    await fit(page);
    await parked(page);
    await expect.configure({ soft: true }).poll(() => undrawn(page, [...others, ...Object.keys(odd)]), { message: 'every box drawn, the odd ones too', timeout: 30_000 }).toEqual([]);
    await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, 'gone_file'))?.rendered?.media?.failed ?? false, { message: 'the gone file: a placeholder', timeout: 90_000 }).toBe(true);
    const settled = asks.length;
    await page.waitForTimeout(5_000);
    expect.soft(asks.length - settled, `the gone file not asked for again once given up (${settled} asks before)`).toBe(0);
  });
});
