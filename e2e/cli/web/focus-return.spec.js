// focus-return (cl-1207; found by cl-1198's Focus fade): on a canvas of 120 pictures, Focus on one and back, every box on screen is drawn
// on every frame of the way back: none blank. The board asked each box's picture at the camera's settled zoom, which in Focus is Focus's:
// a box rebuilt on the way back asked for its file at that zoom, and stood empty until it came. It asks at the canvas's own zoom now, so
// the thumbnail it had is drawn at once.
const { test, expect, canvasIdle, focus, png } = require('./fixtures');
const { fit } = require('./hands');

const COLUMNS = 12, ROWS = 10;
const names = Array.from({ length: COLUMNS * ROWS }, (_, i) => `pic_${String(i).padStart(3, '0')}`);

/** Each frame from now until `stopped`: how many boxes on screen show and have no picture drawn, and which. */
const watch = (page) => page.evaluate(() => {
  const seen = (window.__blanks = { frames: 0, blank: 0, most: 0, first: null, id: 0 });
  const tick = () => {
    const root = document.querySelector('.canvas-root'), inspection = root?.canvasInspection;
    if (inspection) {
      const pane = root.getBoundingClientRect();
      const blank = [];
      for (const name of inspection.names()) {
        const card = inspection.card(name);
        const b = card?.bounds;
        if (!b || b.x + b.width < 0 || b.y + b.height < 0 || b.x > pane.width || b.y > pane.height) continue;
        const drawn = card.rendered;
        if (drawn && drawn.transform.alpha <= 0) continue;
        if (!drawn?.media?.ready) blank.push(name);
      }
      seen.frames++;
      if (blank.length) { seen.blank++; seen.most = Math.max(seen.most, blank.length); seen.first ??= blank.slice(0, 8); }
    }
    seen.id = requestAnimationFrame(tick);
  };
  tick();
});
const stopped = (page) => page.evaluate(() => {
  cancelAnimationFrame(window.__blanks.id);
  return window.__blanks;
});

test('focus-return', async ({ page, project }) => {
  test.setTimeout(300_000);
  for (let at = 0; at < names.length; at += 8) {
    await Promise.all(names.slice(at, at + 8).map((name, k) => {
      const i = at + k;
      return project.import(png((i * 37) % 256, 320, 200), { name, at: { x: (i % COLUMNS) * 360, y: Math.floor(i / COLUMNS) * 320 } });
    }));
  }
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await expect.poll(() => page.evaluate(() => document.querySelector('.canvas-root')?.canvasInspection?.names().length ?? 0), { message: 'all 120 on the canvas', timeout: 60_000 }).toBe(names.length);
  await fit(page);
  // The canvas at rest: every box on screen drawn.
  await watch(page);
  await expect.poll(async () => {
    const now = await stopped(page);
    await watch(page);
    return now.blank;
  }, { message: 'every box drawn at rest', timeout: 120_000, intervals: [1_000] }).toBe(0);
  await stopped(page);

  await test.step('Focus on one, and back: every box on screen drawn on every frame of the way back', async () => {
    await focus(page, names[5 * COLUMNS + 6]);
    await canvasIdle(page);
    await watch(page);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
    await canvasIdle(page);
    const back = await stopped(page);
    test.info().annotations.push({ type: 'back', description: JSON.stringify(back) });
    expect(back.frames, 'frames looked at').toBeGreaterThan(0);
    expect.soft(back.blank, `frames with a box on screen and no picture (most at once ${back.most}, first ${JSON.stringify(back.first)})`).toBe(0);
  });
});
