// dblclick-text (cl-1095): Paul, 11-16: "Because we removed the empty nodes to the left, you can no longer create arbitrary empty nodes anymore
// for text ... there is no way to just start typing text). So I'm thinking you double-click, and it'll create a text node." A double click on
// open canvas places a Text box there, as Tools › Text drops one, and opens it on its words, the caret in them: what's typed is the box's,
// where it was double-clicked, after a reload too. A double click on a box or its name opens that box and makes none; nor does the one that
// leaves a field being typed in, or a click after a drag. Undo takes the new box away. Each step at 1440×900 and 1920×1080, on paper and
// dark, kept for a person to look at (Paul, 11-12: look at your own UI): the empty canvas, the new box with its caret, its words typed, and it
// back on the canvas.
const { mkdirSync } = require('fs');
const { join } = require('path');
const { test, expect, canvasCard, canvasIdle, canvasNames, canvasPoint, canvasVisible, zoomOf } = require('./fixtures');
const { leave } = require('./hands');
const { onCanvas, rowsOf } = require('./geometry');

/** Where Placer puts a box let go at a point of the canvas: its middle under it, its head 16 above (place.ts addTool). */
const placedAt = (flow) => ({ x: Math.round(flow.x - 120), y: Math.round(flow.y - 16) });
/** Open canvas a box's room apart, right of the tools, under the bar, at both sizes. */
const LOOKS = [
  { width: 1440, height: 900, theme: 'paper', point: { x: 560, y: 240 } },
  { width: 1440, height: 900, theme: 'dark', point: { x: 980, y: 240 } },
  { width: 1920, height: 1080, theme: 'paper', point: { x: 560, y: 600 } },
  { width: 1920, height: 1080, theme: 'dark', point: { x: 980, y: 600 } },
];

/** Open canvas at 1920×1080 clear of the four: for what makes no box, and for Undo's. */
const ASIDE = { x: 1400, y: 300 }, UNDONE = { x: 1400, y: 660 };

/** Nothing of the page's but the canvas at a point: no box, no panel, no note. */
const open = (page, { x, y }) => page.evaluate(({ x, y }) => Boolean(document.elementFromPoint(x, y)?.classList.contains('canvas-pane'))
  && !document.querySelector('.canvas-root')?.canvasInspection?.hitTest({ x, y }), { x, y });

test('dblclick-text', async ({ page, project }) => {
  const shots = join(process.env.OUT, 'web', 'dblclick-text');
  mkdirSync(shots, { recursive: true });
  const region = page.getByRole('region', { name: 'Focused asset' });
  const words = region.getByRole('textbox', { name: 'Text', exact: true });
  const names = async () => (await project.assets()).map((asset) => asset.name).sort();
  const made = [];

  for (const { width, height, theme, point } of LOOKS) {
    const tag = `${width}x${height}-${theme}`;
    await test.step(`${tag}: a double click on open canvas, then words typed`, async () => {
      await page.setViewportSize({ width, height });
      await page.evaluate((one) => localStorage.setItem('cozy.theme', one), theme);
      await page.reload();
      await expect(page.locator('.stage')).toBeVisible();
      await canvasIdle(page);
      expect(await open(page, point), `${JSON.stringify(point)} is open canvas`).toBe(true);
      await page.screenshot({ path: join(shots, `empty-${tag}.png`) });
      const was = await names();
      const flow = await onCanvas(page, point);

      await page.mouse.dblclick(point.x, point.y);
      await expect(region, 'the new box opens').toBeVisible();
      const name = await region.getAttribute('data-focused-asset');
      expect(was, 'a box of its own').not.toContain(name);
      await expect(words, 'the caret in its words').toBeFocused();
      await canvasIdle(page);
      await page.screenshot({ path: join(shots, `caret-${tag}.png`), caret: 'initial' });

      const typed = `Words typed where they were wanted, ${width} by ${height} on ${theme}.`;
      await page.keyboard.type(typed);
      await expect(words).toHaveText(typed);
      await page.screenshot({ path: join(shots, `typed-${tag}.png`), caret: 'initial' });
      await expect.poll(async () => (await project.asset(name))?.fields?.text, { message: 'the API holds what was typed' }).toBe(typed);
      expect.soft(await project.asset(name), 'a Text box, as Tools › Text makes one').toMatchObject({ kind: 'text' });
      expect.soft(((await project.asset(name)).tool ?? '') || null, 'no tool').toBeNull();
      expect.soft((await names()).length, 'one box more').toBe(was.length + 1);

      await page.keyboard.press('Escape');
      await expect(region).toHaveCount(0);
      await canvasIdle(page);
      // Where it was double-clicked, at the canvas's own scale, drawn readably and covered by nothing.
      const card = await canvasCard(page, name), r = card.bounds;
      expect.soft(r.x <= point.x && point.x <= r.x + r.width && r.y <= point.y && point.y <= r.y + r.height, `drawn over where it was double-clicked, ${JSON.stringify(point)} in ${JSON.stringify(r)}`).toBe(true);
      const row = (await rowsOf(project, [name]))[name], want = placedAt(flow);
      expect.soft(Math.abs(row.x - want.x) <= 1 && Math.abs(row.y - want.y) <= 1, `its row ${JSON.stringify(row)} is the double click's place, ${JSON.stringify(want)}`).toBe(true);
      expect.soft(await zoomOf(page), 'the canvas at its default scale').toBeCloseTo(1, 2);
      expect.soft(card.bounds.width, 'as wide on screen as a dropped box').toBeGreaterThanOrEqual(200);
      const body = card.body;
      const corners = [[0.5, 0.5], [0.05, 0.05], [0.95, 0.05], [0.05, 0.95], [0.95, 0.95]].map(([fx, fy]) => ({ x: body.x + body.width * fx, y: body.y + body.height * fy }));
      const covered = await page.evaluate((points) => points.filter(({ x, y }) => !document.elementFromPoint(x, y)?.classList.contains('canvas-pane')).map(({ x, y }) => [Math.round(x), Math.round(y), document.elementFromPoint(x, y)?.className]), corners);
      expect.soft(covered, 'nothing of the page over it').toEqual([]);
      await expect.poll(async () => (await canvasCard(page, name))?.rendered?.words, { message: 'its words drawn on it' }).toBe(typed);
      await page.screenshot({ path: join(shots, `placed-${tag}.png`) });
      made.push({ name, typed, row });
    });
  }

  await test.step('after a reload, each where it was with its words', async () => {
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await canvasIdle(page);
    const rows = await rowsOf(project, made.map(({ name }) => name));
    for (const { name, typed, row } of made) {
      await expect.configure({ soft: true }).poll(() => canvasVisible(page, name), { message: `${name} on the canvas` }).toBe(true);
      await expect.configure({ soft: true }).poll(async () => (await canvasCard(page, name))?.rendered?.words, { message: `${name}'s words` }).toBe(typed);
      expect.soft({ x: rows[name].x, y: rows[name].y }, `${name} where it was`).toEqual({ x: row.x, y: row.y });
      expect.soft((await project.asset(name))?.fields?.text, `${name}'s words kept`).toBe(typed);
    }
  });

  const [first] = made;
  const none = async (how) => {
    await page.waitForTimeout(1000);
    expect.soft(await names(), `${how}: no box more`).toEqual(made.map(({ name }) => name).sort());
  };

  await test.step('a double click on a box or its name opens it and makes no box', async () => {
    const body = await canvasPoint(page, first.name);
    await page.mouse.dblclick(body.x, body.y);
    await expect(region, 'on its body').toHaveAttribute('data-focused-asset', first.name);
    await expect.soft(words).toHaveText(first.typed);
    await leave(page);
    await none('on its body');
    const title = await canvasPoint(page, first.name, 'title');
    await page.mouse.dblclick(title.x, title.y);
    await none('on its name');
    await page.keyboard.press('Escape');
    if (await region.count()) await leave(page);
    await canvasIdle(page);
  });

  await test.step('nor the double click that leaves a field being typed in, nor a click after a drag', async () => {
    const title = await canvasPoint(page, first.name, 'title');
    await page.mouse.click(title.x, title.y);
    const field = page.getByRole('textbox', { name: 'Name', exact: true });
    await expect(field, 'its name being typed').toBeFocused();
    expect(await open(page, ASIDE), `${JSON.stringify(ASIDE)} is open canvas`).toBe(true);
    await page.mouse.dblclick(ASIDE.x, ASIDE.y);
    await expect(field).toHaveCount(0);
    await expect(region).toHaveCount(0);
    await none('leaving a field');
    await canvasIdle(page);

    await page.mouse.move(ASIDE.x, ASIDE.y);
    await page.mouse.down();
    await page.mouse.move(ASIDE.x + 30, ASIDE.y, { steps: 4 });
    await page.mouse.move(ASIDE.x + 60, ASIDE.y, { steps: 4 });
    await page.mouse.up();
    await page.mouse.click(ASIDE.x + 60, ASIDE.y);
    await expect(region).toHaveCount(0);
    await none('a click after a drag');
    await canvasIdle(page);
  });

  await test.step('Undo takes the new box away', async () => {
    const spot = UNDONE;
    expect(await open(page, spot), `${JSON.stringify(spot)} is open canvas`).toBe(true);
    await page.mouse.dblclick(spot.x, spot.y);
    await expect(region).toBeVisible();
    const name = await region.getAttribute('data-focused-asset');
    await expect.poll(() => project.asset(name), { message: `${name} made` }).toBeTruthy();
    await page.keyboard.press('Escape');
    await expect(region).toHaveCount(0);
    await canvasIdle(page);
    await page.mouse.move(spot.x, spot.y);
    await page.keyboard.press('ControlOrMeta+z');
    await expect.configure({ soft: true }).poll(async () => (await canvasNames(page)).includes(name), { message: `${name} off the canvas` }).toBe(false);
    await expect.poll(() => project.asset(name), { message: `${name} out of the API` }).toBeUndefined();
  });
});
