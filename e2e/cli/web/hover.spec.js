// hover (Paul, 11-15: "The image also keeps flashing on hover-over"): six pictures of six colours. The pointer goes onto one and off it ten
// times at a hand's pace: in every frame its picture is on the screen (the scene's texture, or the mounted card's decoded <img>), the screen
// shows its colour at its middle through each hover and each leave, its neighbours never blank, and no hover fetches a picture again.
const { test, expect, canvasCard, canvasIdle, canvasVisible, openPoint, png, screenshotPixels } = require('./fixtures');
const { fit } = require('./hands');

const COLOURS = { c1: [190, 40, 40], c2: [40, 150, 60], c3: [40, 80, 200], c4: [200, 160, 30], c5: [150, 50, 170], c6: [30, 160, 170] };
const NAMES = Object.keys(COLOURS);
const TARGET = 'c2';
const TIMES = 10;

/** In the page, every frame: which of `names` has no picture on the screen. */
function sample(names) {
  const seen = (window.__hover = { blank: {}, frames: 0, mark: 'start', id: 0 });
  const look = () => {
    const inspection = document.querySelector('.canvas-root')?.canvasInspection;
    seen.frames++;
    for (const name of names) {
      const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
      const img = native?.checkVisibility() ? native.querySelector('.media img') : null;
      const drawn = inspection?.card(name)?.rendered;
      const shown = Boolean(img?.complete && img.naturalWidth > 0 && img.checkVisibility({ opacityProperty: true })) || Boolean(drawn?.media?.ready && drawn.transform?.alpha > 0);
      if (!shown) (seen.blank[name] ??= []).push(`${seen.mark}@${Math.round(performance.now())}`);
    }
    seen.id = requestAnimationFrame(look);
  };
  seen.id = requestAnimationFrame(look);
}

test('hover', async ({ page, project }) => {
  for (const [i, name] of NAMES.entries()) await project.import(png(0, 320, 200, false, () => COLOURS[name]), { name, at: { x: (i % 3) * 380, y: Math.floor(i / 3) * 320 } });
  for (const name of NAMES) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  const away = await openPoint(page);
  await page.mouse.move(away.x, away.y);
  await expect.poll(async () => (await Promise.all(NAMES.map((name) => canvasCard(page, name)))).every((card) => card?.rendered?.media.ready), { message: 'every picture drawn' }).toBe(true);
  await canvasIdle(page);
  // Each picture's middle on the screen, and a clip around all six for the screenshots.
  const middles = {};
  for (const name of NAMES) {
    const body = (await canvasCard(page, name)).rendered.body;
    middles[name] = { x: body.x + body.width / 2, y: body.y + body.height / 2 };
  }
  const xs = Object.values(middles).map((p) => p.x), ys = Object.values(middles).map((p) => p.y);
  const clip = { x: Math.max(0, Math.min(...xs) - 20), y: Math.max(0, Math.min(...ys) - 20) };
  Object.assign(clip, { width: Math.max(...xs) + 20 - clip.x, height: Math.max(...ys) + 20 - clip.y });
  const shas = Object.fromEntries(await Promise.all(NAMES.map(async (name) => [name, (await project.asset(name)).sha256.replace(/^sha256:/, '')])));
  const fetched = [];
  page.on('request', (request) => { const name = NAMES.find((one) => request.url().includes(shas[one])); if (name) fetched.push(`${name} ${new URL(request.url()).pathname}`); });
  await page.evaluate(`(${sample})(${JSON.stringify(NAMES)})`);

  /** Screenshots back to back for `ms`: each picture's middle, when it isn't its colour (±12). */
  const looks = async (when, ms = 300) => {
    const off = [];
    const end = Date.now() + ms;
    do {
      const read = screenshotPixels(await page.screenshot({ clip }));
      for (const name of NAMES) {
        const pixel = read(middles[name].x - clip.x, middles[name].y - clip.y);
        if (!pixel.every((value, k) => Math.abs(value - COLOURS[name][k]) <= 12)) off.push(`${when}: ${name} ${pixel}`);
      }
    } while (Date.now() < end);
    return off;
  };

  const seenOff = [];
  await test.step(`onto ${TARGET} and off it, ${TIMES} times`, async () => {
    for (let i = 1; i <= TIMES; i++) {
      await page.evaluate((mark) => { window.__hover.mark = mark; }, `on ${i}`);
      await page.mouse.move(middles[TARGET].x, middles[TARGET].y, { steps: 5 });
      seenOff.push(...(await looks(`on ${i}`)));
      await page.waitForTimeout(300);
      await page.evaluate((mark) => { window.__hover.mark = mark; }, `off ${i}`);
      await page.mouse.move(away.x, away.y, { steps: 5 });
      seenOff.push(...(await looks(`off ${i}`)));
      await page.waitForTimeout(300);
    }
  });

  await test.step('its picture, and every neighbour, on the screen throughout, and nothing fetched', async () => {
    const seen = await page.evaluate(() => { cancelAnimationFrame(window.__hover.id); return window.__hover; });
    expect.soft(seen.frames, 'frames looked at').toBeGreaterThan(60);
    expect.soft(Object.fromEntries(Object.entries(seen.blank).map(([name, at]) => [name, `${at.length} frames, first ${at.slice(0, 4).join(', ')}`])), 'frames with a picture missing, by box').toEqual({});
    expect.soft(seenOff.slice(0, 12), `screenshots where a picture's middle isn't its colour (${seenOff.length})`).toEqual([]);
    expect.soft(fetched, 'a hover fetches a picture again').toEqual([]);
  });
});
