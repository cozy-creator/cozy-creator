// move (cl-973 core cell 5), at zoom 1: a box dragged by its head lands where it was let go, the pointer's own move to 2 px, and the API's row
// says the same; a selection moves together, by one move, and one Undo puts it all back; a reload keeps what's left; a drag on open canvas pans
// it and moves no box.
const { test, expect, box, canvasIdle, middle, openPoint, png } = require('./fixtures');
const { camera, drag, drawn, rows, rowsOf } = require('./geometry');

// Pictures 240 wide, a row of three, clear of each other.
const PLACES = { A: { x: 0, y: 0 }, B: { x: 360, y: 0 }, C: { x: 720, y: 0 } };
const NAMES = Object.keys(PLACES);
const near = (a, b, by = 2) => Math.abs(a - b) <= by;

/** Each of `names` moved by `by` from `from` (rows or drawn boxes), to 2 px: what's off, else []. */
const offBy = (now, from, by, names) => names.flatMap((name) => (near(now[name].x - from[name].x, by.x) && near(now[name].y - from[name].y, by.y) ? [] : [`${name}: ${JSON.stringify(from[name])} → ${JSON.stringify(now[name])}, not by ${JSON.stringify(by)}`]));
const placeOf = (held) => Object.fromEntries(Object.entries(held).map(([name, { x, y }]) => [name, { x, y }]));

test('move', { tag: '@engines' }, async ({ page, project }) => {
  for (const [i, name] of NAMES.entries()) await project.import(png(60 + 50 * i), { name, at: PLACES[name] });
  for (const name of NAMES) await expect(box(page, name)).toBeVisible();
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
  expect((await camera(page)).zoom, 'all three fit at 100%').toBeCloseTo(1, 4);
  const head = (name) => middle(box(page, name).locator('.head .nm'));

  let afterOne;
  let drawnAfterOne;
  await test.step('drag A by its head', async () => {
    const [held, shown] = [placeOf(await rowsOf(project, NAMES)), await drawn(page)];
    const from = await head('A');
    const by = { x: 30, y: 260 };
    await drag(page, from, { x: from.x + by.x, y: from.y + by.y });
    await canvasIdle(page);
    await expect.poll(async () => offBy(await rows(project), held, by, ['A']), { message: "A's row moved by the pointer's move" }).toEqual([]);
    expect.soft(offBy(await drawn(page), shown, by, ['A']), 'A drawn where it was let go').toEqual([]);
    expect.soft(offBy(await rows(project), held, { x: 0, y: 0 }, ['B', 'C']), 'nothing else moved').toEqual([]);
    afterOne = placeOf(await rows(project));
    drawnAfterOne = await drawn(page);
  });

  await test.step('drag A and B together', async () => {
    const a = await middle(box(page, 'A').locator('.media').first());
    const b = await middle(box(page, 'B').locator('.media').first());
    await page.mouse.click(a.x, a.y);
    await page.keyboard.down('Shift');
    await page.mouse.click(b.x, b.y);
    await page.keyboard.up('Shift');
    await expect(box(page, 'A')).toHaveAttribute('aria-selected', 'true');
    await expect(box(page, 'B')).toHaveAttribute('aria-selected', 'true');
    const shown = await drawn(page);
    const from = await head('A');
    const by = { x: 60, y: 40 };
    await drag(page, from, { x: from.x + by.x, y: from.y + by.y });
    await canvasIdle(page);
    await expect.poll(async () => offBy(await rows(project), afterOne, by, ['A', 'B']), { message: 'A and B moved by one move' }).toEqual([]);
    expect.soft(offBy(await drawn(page), shown, by, ['A', 'B']), 'both drawn where they were let go').toEqual([]);
    expect.soft(offBy(await rows(project), afterOne, { x: 0, y: 0 }, ['C']), 'C stayed').toEqual([]);
  });

  await test.step('Ctrl+Z', async () => {
    await page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => offBy(await rows(project), afterOne, { x: 0, y: 0 }, NAMES), { message: 'one Undo puts both back' }).toEqual([]);
    await expect.poll(async () => offBy(await drawn(page), drawnAfterOne, { x: 0, y: 0 }, NAMES), { message: 'and draws them there' }).toEqual([]);
  });

  await test.step('reload', async () => {
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    for (const name of NAMES) await expect(box(page, name)).toBeVisible();
    await canvasIdle(page);
    expect.soft(offBy(placeOf(await rows(project)), afterOne, { x: 0, y: 0 }, NAMES), 'the reload keeps them').toEqual([]);
    expect.soft(offBy(await drawn(page), drawnAfterOne, { x: 0, y: 0 }, NAMES), 'drawn where they were').toEqual([]);
  });

  await test.step('drag empty canvas', async () => {
    const [held, was] = [placeOf(await rows(project)), await camera(page)];
    const from = await openPoint(page);
    const by = { x: -120, y: -60 };
    await drag(page, from, { x: from.x + by.x, y: from.y + by.y });
    await canvasIdle(page);
    const now = await camera(page);
    expect.soft({ x: Math.round(now.x - was.x), y: Math.round(now.y - was.y), zoom: now.zoom }, 'the canvas pans with the pointer').toEqual({ x: by.x, y: by.y, zoom: was.zoom });
    expect.soft(offBy(placeOf(await rows(project)), held, { x: 0, y: 0 }, NAMES), 'and no box moves').toEqual([]);
  });
});
