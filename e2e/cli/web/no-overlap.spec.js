// no-overlap (cl-973 core cell 6): no two boxes ever lie on each other, in the DOM or by the API's rows. A box let go over another stays where it
// was let go and pushes that one along the row (Paul, 11-06: "the placement position of the new object should be primary, and then other objects
// should move around it"); boxes added with no place appear clear of the rest; a place another writer sets is kept, never overwritten (cl-800),
// and what it lands on is pushed. Settled, nothing moves for 3 s.
const { test, expect, box, canvasIdle, observeStill, png } = require('./fixtures');
const { drag, drawn, lying, rows, rowsOf } = require('./geometry');

// Pictures 240 wide and 182 tall, a row of three 60 apart.
const ROW = { A: { x: 0, y: 0 }, B: { x: 300, y: 0 }, C: { x: 600, y: 0 } };
const ADDED = ['n1', 'n2', 'n3', 'n4', 'n5'];

/** Settled: nothing moving for 300 ms, then nothing for 3 s more, on nothing else. */
async function settled(page, project, what) {
  await expect.poll(() => observeStill(page, 300), { message: `${what}: the canvas comes to rest` }).toEqual([]);
  expect.soft(await lying(page, project), `${what}: no box on another`).toEqual([]);
  expect.soft(await observeStill(page, 3000), `${what}: nothing moves for 3 s`).toEqual([]);
}

test('no-overlap', async ({ page, project, api }) => {
  for (const [i, name] of Object.keys(ROW).entries()) await project.import(png(50 + 60 * i), { name, at: ROW[name] });
  for (const name of Object.keys(ROW)) await expect(box(page, name)).toBeVisible();
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);

  await test.step('lay out a row of 3 boxes through the API, then let A go over B', async () => {
    const laid = await rowsOf(project, Object.keys(ROW));
    expect.soft(Object.fromEntries(Object.entries(laid).map(([name, { x, y }]) => [name, { x, y }])), 'laid out as asked').toEqual(ROW);
    await settled(page, project, 'the row');
    const zoom = (await page.evaluate(() => new DOMMatrix(getComputedStyle(document.querySelector('.react-flow__viewport')).transform).a));
    // Held by the left end of its head and let go 200 along: A over most of B, the pointer short of B, so B is in the way, not a box A joins.
    const head = await box(page, 'A').locator('.head').boundingBox();
    const from = { x: head.x + 8, y: head.y + head.height / 2 };
    const to = { x: from.x + 200 * zoom, y: from.y };
    expect(await page.evaluate(({ x, y }) => document.elementsFromPoint(x, y).every((el) => !el.closest('.card[data-asset="B"]')), to), 'let go off B').toBe(true);
    await drag(page, from, to);
    await expect.poll(async () => (await rows(project)).A?.x, { message: 'A is let go' }).not.toBe(ROW.A.x);
    await settled(page, project, 'A over B');
    const now = await rows(project);
    // Where it was let go, its middle, at its own width or a little less (Paul, 11-06: "the new object can also of course shrink in size if
    // needed slightly"), as place.js judges a crowded drop: 80% of its width at least.
    const tall = (w) => 32 + (w * 150) / 240;
    const middle = { x: now.A.x + now.A.w / 2, y: now.A.y + tall(now.A.w) / 2 };
    expect.soft(Math.hypot(middle.x - (200 + 120), middle.y - tall(240) / 2) <= 4 && now.A.w >= 0.8 * 240, `A stays where it was let go, its middle at 320,${tall(240) / 2}: ${JSON.stringify(now.A)}`).toBe(true);
    expect.soft(now.B.x, 'B is pushed').not.toBe(ROW.B.x);
    expect.soft(now.B.y, 'along the row').toBe(ROW.B.y);
  });

  await test.step('with the canvas open, add 5 boxes with no at', async () => {
    for (const [i, name] of ADDED.entries()) await project.import(png(20 + 40 * i), { name });
    for (const name of ADDED) await expect(box(page, name)).toBeVisible();
    await rowsOf(project, ADDED);
    await settled(page, project, 'five with no place');
  });

  await test.step('have the CLI place one box on an occupied spot, the way place.js lays boxes out', async () => {
    // All of them in view, then n1 put where C is as place.js lays boxes out: the canvas's PATCH, as the holder (`cozy` has no verb that places a box).
    await page.getByRole('button', { name: /more zoom options$/ }).click();
    await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
    await canvasIdle(page);
    const [held, shown] = [await rows(project), await drawn(page)];
    expect(Object.keys(shown).sort(), 'every box drawn').toEqual([...Object.keys(ROW), ...ADDED].sort());
    const spot = { x: held.C.x, y: held.C.y, w: held.C.w ?? 240 };
    await api.patch(project.at('/canvas'), { cards: { n1: spot } });
    await expect.poll(async () => {
      const now = (await drawn(page)).n1;
      return Math.abs(now.x - shown.n1.x) + Math.abs(now.y - shown.n1.y) > 1;
    }, { message: 'the page draws n1 where it was put' }).toBe(true);
    await settled(page, project, 'n1 on C');
    const now = await rows(project);
    expect.soft({ x: now.n1.x, y: now.n1.y, w: now.n1.w ?? 240 }, "the place it was given is kept, never overwritten").toEqual(spot);
    expect.soft({ x: now.C.x, y: now.C.y }, 'C, under it, is pushed').not.toEqual({ x: held.C.x, y: held.C.y });
  });
});
