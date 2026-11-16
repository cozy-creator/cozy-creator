// rendering (Paul, 11-15: the canvas froze while a box rendered, and a double click on it did nothing): a box rendering on local
// (project/tool.py's slow) whose words mention a picture, so its tile draws that picture behind them (cl-1013's path). A double click on it
// opens it in Focus. While it renders, the canvas never freezes: a wheel pan and a pinch zoom each move the camera and what's on the screen
// with it, and the render's line on the tile goes on changing.
const { randomUUID } = require('crypto');
const { test, expect, canvasCard, canvasIdle, canvasPoint, canvasVisible, fixtureTool, openPoint, png, screenshotPixels } = require('./fixtures');
const { camera } = require('./geometry');
const { fit } = require('./hands');

const SLOW = fixtureTool('tool.py/slow');
const RED = [190, 40, 40];
const BLUE = [40, 90, 200];
const near = (pixel, colour, by = 40) => pixel.every((value, k) => Math.abs(value - colour[k]) <= by);

/** `name`'s drawn body on the screen: the scene's, or the mounted card's picture. */
const bodyOf = (page, name) => page.evaluate((name) => {
  const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
  const media = native?.checkVisibility() ? native.querySelector('.media')?.getBoundingClientRect() : null;
  const card = document.querySelector('.canvas-root')?.canvasInspection?.card(name);
  const b = media ?? card?.rendered?.body ?? card?.body;
  return b ? { x: b.x, y: b.y, width: b.width, height: b.height } : null;
}, name);

/** How many of a 40×25 grid over the canvas differ between two screenshots. */
function changed(a, b, pane) {
  let differ = 0;
  for (let i = 0; i < 40; i++) for (let j = 0; j < 25; j++) {
    const x = pane.x + (pane.width * (i + 0.5)) / 40, y = pane.y + (pane.height * (j + 0.5)) / 25;
    if (a(x, y).reduce((sum, value, k) => sum + Math.abs(value - b(x, y)[k]), 0) > 30) differ++;
  }
  return differ / 1000;
}

test('rendering', async ({ page, project, api }) => {
  await project.import(png(0, 320, 200, false, () => RED), { name: 'ref', at: { x: 0, y: 0 } });
  await project.import(png(0, 320, 200, false, () => BLUE), { name: 'marker', at: { x: 420, y: 0 } });
  await project.add({ name: 'making', tool: SLOW, kind: 'image', fields: { instructions: 'the sea behind <ref>', seconds: 90 }, at: { x: 0, y: 320 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SLOW), { message: 'the project lists its local tool' }).toBe(true);
  for (const name of ['ref', 'marker', 'making']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  await api.call('POST', project.at('/runs'), { names: ['making'], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  await expect.poll(async () => Boolean((await canvasCard(page, 'making'))?.rendered?.progress), { message: 'making renders on the canvas', timeout: 60_000 }).toBe(true);
  await expect.poll(async () => (await canvasCard(page, 'marker'))?.rendered?.media.ready ?? false, { message: "marker's picture drawn" }).toBe(true);

  await test.step('a double click on the rendering box opens it in Focus', async () => {
    const { x, y } = await canvasPoint(page, 'making');
    await page.mouse.dblclick(x, y);
    const region = page.getByRole('region', { name: 'Focused asset' });
    await expect(region).toHaveAttribute('data-focused-asset', 'making');
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
  });

  const pane = await page.locator('.canvas-root').boundingBox();
  for (const [how, move] of [
    ['a wheel pan', async () => { for (let i = 0; i < 3; i++) await page.mouse.wheel(120, 80); }],
    ['a pinch zoom', async () => { await page.keyboard.down('Control'); for (let i = 0; i < 3; i++) await page.mouse.wheel(0, -120); await page.keyboard.up('Control'); }],
  ]) {
    await test.step(`while it renders, ${how} moves the canvas and the screen with it`, async () => {
      const away = await openPoint(page);
      await page.mouse.move(away.x, away.y);
      const [was, wasBody, wasRead] = [await camera(page), await bodyOf(page, 'marker'), screenshotPixels(await page.screenshot())];
      await move();
      await canvasIdle(page);
      const [now, body, read] = [await camera(page), await bodyOf(page, 'marker'), screenshotPixels(await page.screenshot())];
      expect.soft(now.x !== was.x || now.y !== was.y || now.zoom !== was.zoom, `the camera moved: ${JSON.stringify(was)} → ${JSON.stringify(now)}`).toBe(true);
      expect.soft(Math.hypot(body.x - wasBody.x, body.y - wasBody.y) > 20 || Math.abs(body.width / wasBody.width - 1) > 0.05, `marker moved on the screen: ${JSON.stringify(wasBody)} → ${JSON.stringify(body)}`).toBe(true);
      const middle = { x: body.x + body.width / 2, y: body.y + body.height / 2 };
      if (middle.x > pane.x && middle.x < pane.x + pane.width && middle.y > pane.y && middle.y < pane.y + pane.height) {
        expect.soft(near(read(middle.x, middle.y), BLUE), `marker's blue where it's now drawn, not ${read(middle.x, middle.y)}`).toBe(true);
      }
      expect.soft(changed(wasRead, read, pane), 'what the canvas shows changed').toBeGreaterThan(0.02);
    });
  }

  await test.step("and its render's line on the tile goes on changing", async () => {
    const said = new Set();
    const end = Date.now() + 10_000;
    while (Date.now() < end) {
      const drawn = (await canvasCard(page, 'making'))?.rendered;
      said.add(JSON.stringify({ progress: drawn?.progress ?? null, labels: Object.entries(drawn?.labels ?? {}).filter(([key]) => key.startsWith('progress') || key === 'time').map(([, label]) => label.fullText) }));
      await page.waitForTimeout(500);
    }
    expect.soft(said.size, `what its line said over 10 s: ${[...said].join(' | ').slice(0, 600)}`).toBeGreaterThan(1);
  });
});
