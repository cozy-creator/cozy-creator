// timed-list (xs-215, hack audit web.md rows 4 and 16): a box's timed list (a list whose rows have a length that adds up, `Total()`) is
// drawn by its shape, as H3's beats are, whatever the tool calls it. The page knew it only as H3's `beats` (and its prompt as
// `description`), so another tool's `scenes` was a plain list. Paul (11-17): no change in the design language between tools for no reason.
// First, pictures to compare across the change: H3's Shot (main text `summary`) and H3's Upscale (main text `prompt`) in Focus, at rest and
// writing, paper and dark (a prompt's own pictures behind it, cl-938's `summary` branch, can't be reached: what's written is in the frame).
const { test, expect, canvasIdle, focus, fixtureTool, themed, MP4 } = require('./fixtures');
const { fit } = require('./hands');

/** Out of Focus, back on the canvas at rest. */
async function back(page) {
  await page.getByRole('button', { name: 'Back to canvas', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
  await canvasIdle(page);
}

const SCENES = `${fixtureTool('scenes.py')}/scene`;
const H3 = 'minimax-h3/generate';
const UPSCALE = 'minimax-h3/upscale';

test('timed-list', async ({ page, project, api }) => {
  await project.add({ name: 'walk', tool: SCENES, kind: 'video', fields: { summary: 'A walk through town.', scenes: [{ look: 'Morning light.', seconds: 3 }, { look: 'Noon.', seconds: 2 }] }, at: { x: 0, y: 0 } });
  await project.add({ name: 'shot', tool: H3, kind: 'video', fields: { summary: 'A lighthouse at dusk, <walk>.', beats: [{ description: 'Waves against <walk>.', duration: 5 }] }, at: { x: 420, y: 0 } });
  await project.add({ name: 'clip', tool: UPSCALE, kind: 'video', fields: { prompt: 'The street at noon, <walk>.' }, at: { x: 840, y: 0 } });
  for (const name of ['walk', 'shot', 'clip']) await api.post(project.at(`/assets/${name}/versions`), { source: await project.upload(MP4) });
  await expect.poll(async () => (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools.map((tool) => tool.id), { message: 'the tools listed', timeout: 120_000 })
    .toEqual(expect.arrayContaining([SCENES, H3, UPSCALE]));
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  await fit(page);

  await test.step("H3's Shot and Upscale in Focus, at rest and writing, paper and dark: to compare across the change", async () => {
    for (const name of ['shot', 'clip']) {
      for (const theme of ['paper', 'dark']) {
        await project.look(theme);
        await themed(page, theme);
        const region = await focus(page, name);
        await page.screenshot({ path: test.info().outputPath(`${name}-rest-${theme}.png`) });
        await region.getByRole('button', { name: 'New video', exact: true }).click();
        await expect(region.locator('.in-frame')).toBeVisible();
        await page.screenshot({ path: test.info().outputPath(`${name}-writing-${theme}.png`) });
        await back(page);
      }
    }
    await project.look(null);
  });

  await test.step("a timed list called `scenes`: rows of a timed list, as H3's beats", async () => {
    const region = await focus(page, 'walk');
    await region.getByRole('button', { name: 'New video', exact: true }).click();
    const list = region.locator('.beats-edit[data-field="scenes"]');
    await expect(list, 'its scenes shown while its words are written').toBeVisible();
    await expect(list.locator(':scope > .beat.beat-accordion'), 'each scene a row of the timed list, opened by its heading, as H3\'s beats').toHaveCount(2);
    await expect.soft(list.locator('.beat-toggle').first(), 'a row\'s heading says its length').toContainText('3');
    for (const theme of ['paper', 'dark']) {
      await project.look(theme);
      await themed(page, theme);
      await page.screenshot({ path: test.info().outputPath(`timed-list-${theme}.png`) });
    }
    await project.look(null);
  });
});
