// shape-names (#4084's review): a box's shape is named by Calcifer's standard ratios from the board's first frame, with no tool list read.
// The page holds them as Calcifer's own constant (calcifer.ts `STANDARD_RATIOS`, written from `tool::STANDARD_RATIOS`), so every tool
// list is held here until the board is drawn. When they came with the project's tool list, which local code can make slow, a shape was
// named by its simplest whole ratio until it came (9:16 said 4:7). Pictures of the board as it opens, paper and dark.
const { test, expect, canvasIdle, canvasLabel, canvasVisible, png, themed } = require('./fixtures');
const { fit } = require('./hands');

test('shape-names', async ({ page, project }) => {
  // 9:16, and Scope's 2.35:1, which the page says as 2.39:1 (ratios.ts `ALSO`).
  await project.import(png(80, 180, 320), { name: 'tall', at: { x: 0, y: 0 } });
  await project.import(png(160, 470, 200), { name: 'scope', at: { x: 300, y: 0 } });
  for (const name of ['tall', 'scope']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
  await fit(page);

  // Every tool list, held until the board is drawn and read.
  const held = [];
  const TOOLS = /\/v1\/tools(\?|$)/;
  await page.route(TOOLS, (route) => { held.push(route); });
  const says = (name) => canvasLabel(page, name, 'shape');

  await test.step('reloaded, every tool list held: each box named by the standard ratios as the board first draws', async () => {
    await page.reload();
    for (const name of ['tall', 'scope']) await expect.poll(() => canvasVisible(page, name)).toBe(true);
    await canvasIdle(page);
    test.info().annotations.push({ type: 'held tool lists', description: String(held.length) });
    test.info().annotations.push({ type: 'names as the board draws', description: JSON.stringify({ tall: await says('tall'), scope: await says('scope') }) });
    expect.soft(await says('tall'), '9:16').toBe('9:16');
    expect.soft(await says('scope'), "2.35:1, Scope's 2.39:1").toBe('2.39:1');
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await themed(page, theme);
      await page.mouse.move(2, 2);
      await page.waitForTimeout(400);
      await page.screenshot({ path: test.info().outputPath(`first-${theme}.png`) });
    }
    await project.look(null);
  });

  await test.step('the tool lists let go: the names stay', async () => {
    await page.unroute(TOOLS);
    for (const route of held) await route.continue().catch(() => {});
    await canvasIdle(page);
    await expect.poll(() => says('tall')).toBe('9:16');
    await expect.poll(() => says('scope')).toBe('2.39:1');
  });
});
