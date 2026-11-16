// resize (Paul, 11-15: "grab an image, and try to resize it, it only lets you go so far, and then stops you, and then the image flashes, and
// then it lets you resize it again, and then it zooms back to its original size… The image also keeps flashing on hover-over"): a picture
// box's corner pulled in 30 steps across the canvas and past its edge grows at every step until the widest a box may be; let go, that size
// is saved and stays; and its picture is on the screen in every frame, hovered, pulled, let go and left, never fetched again by a hover.
const { test, expect, canvasCard, canvasIdle, canvasVisible, cornerPoint, hoverBox, openPoint, png } = require('./fixtures');
const { rows } = require('./geometry');
const { fit } = require('./hands');

const NAME = 'photo';
/** The widest a box is pulled to (Card.tsx WIDEST), in canvas units. */
const WIDEST = 1600;

test('resize', async ({ page, project }) => {
  await project.import(png(120, 640, 400), { name: NAME, at: { x: 0, y: 0 } });
  await project.import(png(200, 320, 200), { name: 'other', at: { x: 0, y: 520 } });
  for (const name of [NAME, 'other']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);
  await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
  await canvasIdle(page);
  await expect.poll(async () => (await canvasCard(page, NAME))?.rendered?.media.ready ?? false, { message: 'its picture drawn' }).toBe(true);
  const sha = (await project.asset(NAME)).sha256.replace(/^sha256:/, '');
  // The scene's texture for it, where the canvas says which (rendered.media.texture); null while its card is mounted over it.
  const texture = async () => (await canvasCard(page, NAME))?.rendered?.media?.texture;
  const unhovered = await texture();
  const fetched = [];
  page.on('request', (request) => { if (request.url().includes(sha)) fetched.push(new URL(request.url()).pathname); });

  // Every frame from here: whether its picture is on the screen (the scene's decoded texture, or the mounted card's decoded <img>), and
  // its width there.
  await page.evaluate((name) => {
    const frames = (window.__resize = { frames: [], id: 0 });
    const look = () => {
      const inspection = document.querySelector('.canvas-root')?.canvasInspection;
      const native = document.querySelector(`.canvas-node:has(.card[data-asset=${JSON.stringify(name)}])`);
      const img = native?.querySelector('.media img');
      const mounted = Boolean(native?.checkVisibility());
      const card = inspection?.card(name), drawn = card?.rendered;
      const nativeShown = mounted && Boolean(img?.complete && img.naturalWidth > 0 && img.checkVisibility({ opacityProperty: true }));
      const sceneShown = Boolean(drawn?.media?.ready && drawn.transform?.alpha > 0);
      frames.frames.push({ at: Math.round(performance.now()), mounted, shown: nativeShown || sceneShown, width: (card?.bounds.width ?? 0) / (inspection?.camera().zoom ?? 1) });
      frames.id = requestAnimationFrame(look);
    };
    frames.id = requestAnimationFrame(look);
  }, NAME);
  const mark = (what) => page.evaluate((what) => window.__resize.frames.push({ mark: what }), what);
  /** Its width now, in canvas units, as the last frame saw it. */
  const width = () => page.evaluate(() => Math.round(window.__resize.frames.filter((one) => !one.mark).at(-1).width * 10) / 10);

  let before, grip;
  await test.step('hovered and left: the same picture, not made again', async () => {
    if (unhovered === undefined) return test.info().annotations.push({ type: 'skipped', description: 'the canvas names no texture (rendered.media.texture)' });
    await hoverBox(page, NAME);
    const away = await openPoint(page);
    await page.mouse.move(away.x, away.y);
    await expect(page.locator(`.canvas-node:has(.card[data-asset="${NAME}"])`)).toHaveCount(0);
    await expect.poll(texture, { message: 'its texture back, the same one' }).toBe(unhovered);
  });

  await test.step('hovered: its card, with its picture, in every frame, and nothing fetched', async () => {
    await mark('hover');
    await hoverBox(page, NAME);
    await page.waitForTimeout(600);
    // Its corner: the old canvas's hover card's grip, the new canvas's drawn one, both shown as it's hovered.
    grip = await cornerPoint(page, NAME);
    expect(grip, 'its corner').toBeTruthy();
    before = await width();
    expect.soft(fetched, 'a hover fetches its picture again (the hover card loads its own copy: cl-1028 PR 2)').toEqual([]);
  });

  const widths = [];
  await test.step('its corner pulled in 30 steps, past the canvas: wider at every step until the widest', async () => {
    await mark('pull');
    const from = grip;
    const to = { x: page.viewportSize().width + 60, y: from.y + 120 };
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    for (let i = 1; i <= 30; i++) {
      await page.mouse.move(from.x + ((to.x - from.x) * i) / 30, from.y + ((to.y - from.y) * i) / 30);
      await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
      widths.push(await width());
    }
    const stalled = widths.flatMap((width, i) => (i > 0 && !(width > widths[i - 1]) && widths[i - 1] < WIDEST - 1 ? [{ step: i + 1, from: widths[i - 1], to: width }] : []));
    expect.soft(stalled, `its width at each step: ${widths.join(', ')}`).toEqual([]);
    expect.soft(widths.at(-1), 'wider than it began').toBeGreaterThan(before);
  });

  await test.step('let go: its size saved, and it stays', async () => {
    await mark('release');
    await page.mouse.up();
    const pulled = widths.at(-1);
    await expect.poll(async () => Math.abs(((await rows(project))[NAME]?.w ?? 0) - pulled) <= 2, { message: `its width ${pulled} saved, not ${before}` }).toBe(true);
    await page.waitForTimeout(1000);
    const shown = await width();
    expect.soft(Math.abs(shown - pulled), `still ${pulled} wide on the screen, not ${Math.round(shown)}`).toBeLessThanOrEqual(2);
  });

  await test.step('left: its picture stays on the screen, and nothing is fetched', async () => {
    const was = fetched.length;
    await mark('leave');
    const away = await openPoint(page);
    await page.mouse.move(away.x, away.y);
    await expect(page.locator(`.canvas-node:has(.card[data-asset="${NAME}"])`)).toHaveCount(0);
    await page.waitForTimeout(600);
    const frames = await page.evaluate(() => { cancelAnimationFrame(window.__resize.id); return window.__resize.frames; });
    // By phase, the frames with no picture on the screen.
    const phases = {};
    let phase = 'start';
    for (const frame of frames) {
      if (frame.mark) phase = frame.mark;
      else if (!frame.shown) (phases[phase] ??= []).push(frame.at);
    }
    expect.soft(frames.filter((frame) => !frame.mark).length, 'frames looked at').toBeGreaterThan(30);
    expect.soft(phases, `frames with no picture on the screen, by phase (of ${frames.length}; cl-1016, cl-1028 PR 2)`).toEqual({});
    expect.soft(fetched.slice(was), 'leaving fetches its picture again').toEqual([]);
  });
});
