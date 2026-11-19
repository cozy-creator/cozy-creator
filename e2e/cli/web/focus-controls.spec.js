// focus-controls (xs-215, hack audit web.md row 15): one `isControl` decides what a press, a drag, a double-click or the wheel in Focus's
// frame belongs to. Every kind of control the old lists named acts as a control (its wheel doesn't zoom the frame, a drag on it doesn't pan
// it, a double-click on it doesn't fit it), and what's looked at (a picture, a video) still zooms. The lists differed: Focus's double-click
// list lacked audio, sliders and the timeline.
const { test, expect, canvasIdle, fixtureTool, focus, middle, png, MP4 } = require('./fixtures');
const { fit } = require('./hands');

const SEGMENTS = fixtureTool('segments.py/segments');

/** The frame's camera, as drawn: its transform, and whether it's zoomed in. */
const camera = (region) => region.evaluate((element) => {
  const transform = element.querySelector('.focus-image-transform');
  return { transform: transform ? getComputedStyle(transform).transform : null, zoomed: element.querySelector('.focus-viewer')?.hasAttribute('data-zoomed') ?? false };
});

async function back(page) {
  await page.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
  await canvasIdle(page);
}

/** The wheel over `target`, then a drag from it (`drag`: unless its own drag is something else, a corner's reshaping): the frame's camera
 *  stays as it was. */
async function actsAsControl(page, region, target, what, { drag = true } = {}) {
  await target.scrollIntoViewIfNeeded().catch(() => {});
  const before = await camera(region);
  const at = await middle(target);
  await page.mouse.move(at.x, at.y);
  await page.mouse.wheel(0, -400);
  await page.waitForTimeout(300);
  expect.soft(await camera(region), `${what}: the wheel over it doesn't zoom the frame`).toEqual(before);
  if (!drag) return;
  await page.mouse.down();
  await page.mouse.move(at.x + 60, at.y + 40, { steps: 6 });
  await page.mouse.up();
  await page.waitForTimeout(200);
  expect.soft(await camera(region), `${what}: a drag from it doesn't pan the frame`).toEqual(before);
}

test('focus-controls', async ({ page, project, api }) => {
  const { tools } = await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}&q=qwen`);
  const qwen = tools.find((tool) => tool.package === 'qwen-image-2' && tool.makes === 'image' && tool.fields.some((field) => field.type === 'prompt'));
  expect(qwen, 'the Hub lists Qwen Image 2 with a prompt').toBeTruthy();
  await project.import(png(70, 2400, 1600), { name: 'wide', at: { x: 0, y: 0 } });
  await project.add({ name: 'harbour', tool: qwen.id, kind: 'image', fields: { instructions: 'a lighthouse at dusk' }, at: { x: 420, y: 0 } });
  await api.post(project.at('/assets/harbour/versions'), { source: await project.upload(png(150, 1200, 800)) });
  await project.import(MP4, { name: 'clip', at: { x: 840, y: 0 } });
  await project.add({ name: 'coast', tool: SEGMENTS, kind: 'video', fields: { summary: 'Along the coast.', beats: [{ description: 'The cliffs.', duration: 8 }, { description: 'The harbour.', duration: 8 }], wait: 8 }, at: { x: 0, y: 900 } });
  await expect.poll(async () => (await project.asset('harbour')).versions, { message: 'harbour has its take' }).toBe(1);
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.some((tool) => tool.local && tool.id === SEGMENTS), { message: 'the project lists its local tool', timeout: 120_000 }).toBe(true);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);

  await test.step("a picture zoomed in: its zoom toolbar is a control; the picture itself zooms, and a double-click on it fits it", async () => {
    const region = await focus(page, 'wide');
    await region.getByRole('button', { name: 'Zoom in image', exact: true }).click();
    await expect.poll(async () => (await camera(region)).zoomed, { message: 'zoomed in' }).toBe(true);
    await actsAsControl(page, region, region.locator('.focus-image-zoom'), 'the zoom toolbar');
    // Zoom in zooms with each click: a double-click on it is its two clicks, never the frame's fit.
    await region.getByRole('button', { name: 'Zoom in image', exact: true }).dblclick();
    expect.soft((await camera(region)).zoomed, 'a double-click on Zoom in doesn\'t fit the frame').toBe(true);
    const before = await camera(region);
    const picture = await middle(region.locator('.focus-image-transform'));
    await page.mouse.move(picture.x, picture.y);
    await page.mouse.wheel(0, -400);
    await expect.poll(async () => (await camera(region)).transform, { message: 'the wheel over the picture zooms it' }).not.toBe(before.transform);
    await region.locator('.focus-image-transform').dblclick();
    await expect.poll(async () => (await camera(region)).zoomed, { message: 'a double-click on the picture fits it' }).toBe(false);
    await back(page);
  });

  await test.step('a picture being written: its words and its corner are controls (the rest of the frame is the picture)', async () => {
    const region = await focus(page, 'harbour');
    await region.getByRole('button', { name: 'Its next take', exact: true }).click();
    await expect(region.locator('.in-frame')).toBeVisible();
    await actsAsControl(page, region, region.locator('.in-frame .mention-text'), 'its words');
    await actsAsControl(page, region, region.locator('.corner'), 'its corner', { drag: false });
    await back(page);
  });

  await test.step("a click on a gap of Focus's fields panel goes on up to the page: what a drawer's click-off listens for (Paul's rule)", async () => {
    const region = await focus(page, 'harbour');
    await region.getByRole('button', { name: 'Its next take', exact: true }).click();
    const panel = region.locator('.here-under');
    await expect(panel).toBeVisible();
    // A point on the panel itself: none of its controls, labels or fields.
    const gap = await panel.evaluate((el) => {
      const r = el.getBoundingClientRect();
      for (let y = r.top + 2; y < r.bottom - 2; y += 3) for (let x = r.left + 2; x < r.right - 2; x += 6) {
        const hit = document.elementFromPoint(x, y);
        if (hit && el.contains(hit) && !hit.closest('button, input, select, textarea, label, a, [contenteditable], [role], .seg, .field, .ctl-name')) return { x, y };
      }
      return null;
    });
    expect(gap, 'a gap in the fields panel').toBeTruthy();
    // The Assets and Runs drawers close on a click that reaches the window and began outside them (Shell's click-off).
    await page.evaluate(() => { window.heardClicks = 0; window.addEventListener('click', () => { window.heardClicks += 1; }); });
    await page.mouse.click(gap.x, gap.y);
    expect(await page.evaluate(() => window.heardClicks), 'the click reached the page, as a drawer open would hear it').toBe(1);
    await back(page);
  });

  await test.step('a video: its transport under it is a control; the video itself zooms', async () => {
    const region = await focus(page, 'clip');
    await expect(region.locator('.focus-under')).toBeVisible();
    await actsAsControl(page, region, region.locator('.focus-under'), 'its transport');
    const video = await middle(region.locator('video.live'));
    await page.mouse.move(video.x, video.y);
    await page.mouse.wheel(0, -400);
    await expect.poll(async () => (await camera(region)).zoomed, { message: 'the wheel over the video zooms it' }).toBe(true);
    await back(page);
  });

  await test.step("a render's glimpse in Focus is what's looked at: it zooms, as the video it becomes does", async () => {
    const region = await focus(page, 'coast');
    await region.getByRole('button', { name: 'Render', exact: true }).click();
    const making = region.locator('.here-side .take.making').first();
    await expect(making, 'its take on its way').toBeVisible({ timeout: 120_000 });
    await expect.poll(async () => (await making.locator('video.take-glimpse').count()) > 0, { message: 'segment 1 glimpsed on its tile', timeout: 120_000 }).toBe(true);
    await making.click();
    const glimpse = region.locator('.glimpse video');
    await expect(glimpse, 'its glimpse open in the frame').toBeVisible({ timeout: 30_000 });
    const at = await middle(glimpse);
    await page.mouse.move(at.x, at.y);
    await page.mouse.wheel(0, -400);
    await expect.poll(async () => (await camera(region)).zoomed, { message: 'the wheel over the glimpse zooms it' }).toBe(true);
    await back(page);
  });
});
