// svg (Paul, 11-14: "we can use SVGs, but also block scripts"; cl-985): a tool's picture that is an SVG holding a <script> and an onload
// (project/svg.py, as a package may make one), rendered, is drawn as its take on the canvas and in Focus, and neither its
// script nor its onload asks for anything.
const { test, expect, canvasCard, canvasVisible, focus, fixtureTool, screenshotPixels } = require('./fixtures');

const SVG = fixtureTool('svg.py/svg');
const NAME = 'drawing';
const RED = [204, 51, 51];
const RUN_MS = 120_000;

/** How many of a 5×5 grid over the middle of `rect` are the SVG's red (#c33). */
function redShare(read, rect) {
  let red = 0;
  for (let i = 1; i <= 5; i++) for (let j = 1; j <= 5; j++) {
    const pixel = read(rect.x + (rect.width * (0.2 + 0.1 * i)), rect.y + (rect.height * (0.2 + 0.1 * j)));
    if (pixel.every((value, k) => Math.abs(value - RED[k]) <= 40)) red++;
  }
  return red / 25;
}

test('svg', async ({ page, project, api, cli }) => {
  const asked = [];
  page.context().on('request', (request) => { if (request.url().includes('svg=')) asked.push(request.url()); });
  await project.add({ name: NAME, tool: SVG, kind: 'image', at: { x: 0, y: 0 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SVG), { message: 'the project lists its local tool' }).toBe(true);
  await expect.poll(() => canvasVisible(page, NAME), { message: 'its box on the canvas' }).toBe(true);

  await test.step('rendered (cozy render)', async () => {
    const rendered = await cli('--json', 'render', NAME, '--on', 'local');
    expect(rendered.code, `cozy render: ${rendered.stderr.slice(-400)}`).toBe(0);
    await expect.poll(async () => (await project.asset(NAME))?.versions ?? 0, { message: 'its take', timeout: RUN_MS }).toBe(1);
  });

  await test.step('drawn on the canvas as its take', async () => {
    await page.mouse.move(4, 600);
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
