// unseen (cl-1105), Paul 11-16: "also when I produce new outputs, make sure that, on the node where they appeared, they are marked with a
// green dot in the top left, both the node that has uninspected new outputs, and when in focus, on the images /thumbnails to the right that
// haven't been viewed yet." A box with nothing approved renders three times (near white, near black, grey): all are unseen, its green dot is
// drawn at its top left and stays through a pan, a zoom and a reload, and hovering it says how many. Focus shows the newest, so that one is
// seen, and the others' thumbnails keep their dots until each is shown, then the box's goes. Rendering again beside an unseen version draws
// the green dot and the blue one side by side.
// Calcifer has one person, its OS user, so no second person's unseen is checked. Pictures at 1440×900 and 1920×1080, paper and dark, in
// $OUT/web/unseen.
const { randomUUID } = require('crypto');
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasCard, canvasIdle, focus, fixtureTool, openPoint, png, screenshotPixels } = require('./fixtures');

const SHADE = `${fixtureTool('shades.py')}/shade`;
const SIZES = [[1440, 900], [1920, 1080]];
const SHOTS = join(process.env.OUT || '.', 'web', 'unseen');
const softly = expect.configure({ soft: true });
const focused = (page) => page.getByRole('region', { name: 'Focused asset' });
const tile = (region, number) => region.getByRole('button', { name: `version ${number}`, exact: true });

async function fit(page) {
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
}

async function theme(page, which) {
  await page.evaluate((dark) => localStorage.setItem('cozy.theme', dark ? 'dark' : 'paper'), which === 'dark');
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await canvasIdle(page);
}

/** Each size of the page as it is now, `what-<w>x<h>-<theme>.png`. */
async function shoot(page, what, which, prepare = async () => {}) {
  mkdirSync(SHOTS, { recursive: true });
  for (const [width, height] of SIZES) {
    await page.setViewportSize({ width, height });
    await prepare();
    await canvasIdle(page);
    await page.screenshot({ path: join(SHOTS, `${what}-${width}x${height}-${which}.png`) });
  }
}

/** What the canvas draws of `name`'s dots, in screen pixels, with its box's edges. */
async function dots(page, name) {
  const card = await canvasCard(page, name);
  const shown = card?.rendered;
  return shown ? { unseen: shown.unseen, state: shown.state, dot: shown.unseenDot, dots: shown.stateDot, bounds: card.bounds, body: shown.body, title: shown.labels.title } : null;
}

/** The colour the page draws at the middle of `box`, and the green the theme names. */
async function colourAt(page, box) {
  const read = screenshotPixels(await page.screenshot({ scale: 'css' }));
  const green = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--state-unseen').trim());
  const want = [1, 3, 5].map((at) => parseInt(green.slice(at, at + 2), 16));
  return { got: read(box.x + box.width / 2, box.y + box.height / 2), want };
}

test('unseen', async ({ page, project, api }) => {
  test.setTimeout(420_000);
  const render = (name) => api.call('POST', project.at('/runs'), { names: [name], on: ['local'] }, { headers: { 'Idempotency-Key': randomUUID() } });
  const versions = async (name) => { const row = await project.asset(name); return row?.making?.length ? -1 : row?.versions ?? 0; };
  const unseen = async (name) => (await project.asset(name))?.unseen ?? [];

  await project.add({ name: 'grey', tool: SHADE, kind: 'image', fields: { instructions: 'A light grey.', shade: 0.94 }, at: { x: 0, y: 0 } });
  for (const [i, shade] of [40, 200, 120].entries()) await project.import(png(shade), { name: `still_${i + 1}`, at: { x: 420 * (i + 1), y: 0 } });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the shade tool listed', timeout: 120_000 })
    .toContain(SHADE);
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);

  await test.step('three versions land on a box with nothing approved', async () => {
    for (const [number, shade] of [[1, 0.94], [2, 0.06], [3, 0.5]]) {
      await api.patch(project.at('/assets/grey'), { fields: { instructions: `A grey of ${shade}.`, shade } });
      await render('grey');
      await expect.poll(() => versions('grey'), { message: `version ${number}`, timeout: 150_000 }).toBe(number);
    }
    expect((await project.asset('grey')).approved ?? null, 'nothing approved').toBeNull();
    expect(await unseen('grey'), 'all unseen').toEqual([1, 2, 3]);
  });

  await test.step('its green dot, at its top left, through a pan, a zoom and a reload', async () => {
    await expect.poll(async () => (await dots(page, 'grey'))?.dot ?? null, { message: 'the green dot drawn' }).not.toBeNull();
    const drawn = await dots(page, 'grey');
    expect.soft(drawn.unseen, 'it counts 3').toBe(3);
    // The box's top left: at its left edge, in its name's row above the picture, before the name.
    const middle = drawn.dot.y + drawn.dot.height / 2;
    expect.soft(Math.abs(drawn.dot.x - drawn.bounds.x), `at its left edge: ${JSON.stringify(drawn)}`).toBeLessThan(drawn.dot.width);
    expect.soft(drawn.dot.y + drawn.dot.height, 'above its picture').toBeLessThanOrEqual(drawn.body.y);
    expect.soft(middle > drawn.title.y && middle < drawn.title.y + drawn.title.height, "in its name's row").toBe(true);
    expect.soft(drawn.dot.x + drawn.dot.width, 'before its name').toBeLessThan(drawn.title.x);
    expect.soft(drawn.dot.width, 'big enough to see').toBeGreaterThanOrEqual(8);
    const { got, want } = await colourAt(page, drawn.dot);
    expect.soft(Math.max(...got.map((value, i) => Math.abs(value - want[i]))), `green on screen: ${got} vs ${want}`).toBeLessThanOrEqual(40);
    for (const name of ['still_1', 'still_2', 'still_3']) expect.soft((await dots(page, name))?.dot ?? null, `${name} was imported: no dot`).toBeNull();

    await page.mouse.move(drawn.dot.x + drawn.dot.width / 2, drawn.dot.y + drawn.dot.height / 2);
    await softly.poll(() => page.locator('.canvas-root').getAttribute('title'), { message: 'hovered, it says how many' }).toBe('3 new versions, not looked at');

    const open = await openPoint(page);
    await page.mouse.move(open.x, open.y);
    await page.mouse.wheel(0, 240);
    await canvasIdle(page);
    await page.getByRole('button', { name: 'Zoom out', exact: true }).click();
    await canvasIdle(page);
    await page.waitForTimeout(3000);
    expect.soft((await dots(page, 'grey'))?.dot ?? null, 'still drawn after a pan and a zoom').not.toBeNull();
    expect.soft(await unseen('grey'), 'drawing it looks at nothing').toEqual([1, 2, 3]);

    await fit(page);
    await shoot(page, 'canvas', 'paper', () => fit(page));
    await theme(page, 'dark');
    await fit(page);
    await expect.poll(async () => (await dots(page, 'grey'))?.unseen ?? 0, { message: 'after a reload' }).toBe(3);
    expect.soft((await dots(page, 'grey')).dot, 'its dot after a reload').not.toBeNull();
    await shoot(page, 'canvas', 'dark', () => fit(page));
    expect.soft(await unseen('grey'), 'still unseen').toEqual([1, 2, 3]);
  });

  await test.step('Focus: the one shown is seen, the others keep their dots until shown', async () => {
    const region = await focus(page, 'grey');
    await expect.soft(region.getByLabel('Take', { exact: true })).toHaveText('v3');
    await softly.poll(() => unseen('grey'), { message: 'the one shown is seen' }).toEqual([1, 2]);
    const mark = (number) => tile(region, number).getByRole('img', { name: 'Not looked at yet', exact: true });
    for (const number of [1, 2]) await expect.soft(mark(number), `version ${number} has its dot`).toBeVisible();
    await expect.soft(mark(3), 'version 3 has none').toHaveCount(0);
    for (const number of [1, 2]) {
      const ring = await mark(number).boundingBox(), thumb = await tile(region, number).boundingBox();
      expect.soft(ring && thumb && ring.x - thumb.x < 12 && ring.y - thumb.y < 12, `at version ${number}'s top left: ${JSON.stringify({ ring, thumb })}`).toBe(true);
    }
    await shoot(page, 'focus', 'dark');
    await theme(page, 'paper');
    await expect(focused(page)).toHaveAttribute('data-focused-asset', 'grey');
    await expect.soft(mark(1), 'still there after a reload').toBeVisible();
    await shoot(page, 'focus', 'paper');

    await tile(focused(page), 1).click();
    await softly.poll(() => unseen('grey'), { message: 'version 1 shown' }).toEqual([2]);
    await expect.soft(mark(1), 'its dot goes').toHaveCount(0);
    await tile(focused(page), 2).click();
    await softly.poll(() => unseen('grey'), { message: 'version 2 shown' }).toEqual([]);
    await expect.soft(mark(2), 'its dot goes').toHaveCount(0);
    await focused(page).getByRole('button', { name: 'Back to canvas', exact: true }).click();
    await expect(focused(page)).toHaveCount(0);
    await canvasIdle(page);
    await softly.poll(async () => (await dots(page, 'grey'))?.dot ?? null, { message: "the box's dot goes" }).toBeNull();
  });

  await test.step('rendering beside an unseen version: green and blue together', async () => {
    await api.patch(project.at('/assets/grey'), { fields: { instructions: 'A light grey again.', shade: 0.8, seconds: 0 } });
    await render('grey');
    await expect.poll(() => versions('grey'), { message: 'version 4', timeout: 120_000 }).toBe(4);
    await api.patch(project.at('/assets/grey'), { fields: { instructions: 'A slow grey.', seconds: 25 } });
    await render('grey');
    await expect.poll(async () => { const now = await dots(page, 'grey'); return now && `${now.state} ${now.unseen}`; }, { message: 'running, one unseen', timeout: 120_000 }).toBe('running 1');
    const both = await dots(page, 'grey');
    expect.soft(both.dots.width, `two dots side by side: ${JSON.stringify(both)}`).toBeGreaterThan(both.dot.width * 2);
    expect.soft(Math.abs(both.dot.x - both.dots.x), 'the green one first').toBeLessThan(1);
    await page.mouse.move(both.dots.x + both.dots.width * 0.75, both.dots.y + both.dots.height / 2);
    await softly.poll(() => page.locator('.canvas-root').getAttribute('title'), { message: 'hovered, they say both' }).toBe('Rendering. 1 new version, not looked at');
    await page.mouse.move((await openPoint(page)).x, (await openPoint(page)).y);
    await shoot(page, 'running', 'paper', () => fit(page));
    await theme(page, 'dark');
    await fit(page);
    await shoot(page, 'running', 'dark', () => fit(page));
    await expect.poll(() => versions('grey'), { message: 'version 5', timeout: 120_000 }).toBe(5);
    expect.soft(await unseen('grey'), 'both unseen').toEqual([4, 5]);
  });
});
