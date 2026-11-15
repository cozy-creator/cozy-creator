// svg (Paul, 11-14: "we can use SVGs, but also block scripts"; cl-985): an SVG holding a <script> and an onload, imported as a file, is
// drawn as its picture on the canvas and in Focus, and neither its script nor its onload asks for anything.
const { test, expect, canvasCard, canvasVisible, focus, screenshotPixels } = require('./fixtures');

const NAME = 'drawing';
const RED = [204, 51, 51];
const SVG = Buffer.from(
  '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="480" onload="fetch(\'/v1/status?svg=onload\')">' +
  '<rect width="640" height="480" fill="#cc3333"/><script>fetch("/v1/status?svg=script")</script></svg>',
);

/** How many of a 5×5 grid over the middle of `rect` are the SVG's red. */
function redShare(read, rect) {
  let red = 0;
  for (let i = 1; i <= 5; i++) for (let j = 1; j <= 5; j++) {
    const pixel = read(rect.x + (rect.width * (0.2 + 0.1 * i)), rect.y + (rect.height * (0.2 + 0.1 * j)));
    if (pixel.every((value, k) => Math.abs(value - RED[k]) <= 40)) red++;
  }
  return red / 25;
}

test('svg', async ({ page, project }) => {
  const asked = [];
  page.context().on('request', (request) => { if (request.url().includes('svg=')) asked.push(request.url()); });
  await project.import(SVG, { name: NAME, at: { x: 0, y: 0 } });
  await expect.poll(() => canvasVisible(page, NAME), { message: 'its box on the canvas' }).toBe(true);

  await test.step('drawn on the canvas as its picture', async () => {
    await expect.poll(async () => {
      const media = (await canvasCard(page, NAME))?.rendered?.media;
      return media ? { ready: media.ready, failed: media.failed } : null;
    }, { message: 'its picture decoded and drawn', timeout: 30_000 }).toEqual({ ready: true, failed: false });
    const body = (await canvasCard(page, NAME)).rendered.body;
    expect.soft(redShare(screenshotPixels(await page.screenshot()), body), 'its red on the screen').toBeGreaterThanOrEqual(0.8);
  });

  await test.step('in Focus too', async () => {
    const region = await focus(page, NAME);
    const picture = region.locator('.media img').first();
    await expect.poll(() => picture.evaluate((img) => img.complete && img.naturalWidth > 0).catch(() => false), { message: 'its picture in Focus' }).toBe(true);
    expect.soft(redShare(screenshotPixels(await page.screenshot()), await picture.boundingBox()), 'its red in Focus').toBeGreaterThanOrEqual(0.8);
  });

  await test.step('its script and onload asked for nothing', async () => {
    expect(asked).toEqual([]);
  });
});
