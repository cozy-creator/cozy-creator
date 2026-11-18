// se-141 / se-164's proof (release-gate/sessions/video-mentions.sh; never merged). Paul, 11-17: "It should have a reference text like <Am>
// like every other model, and then it shows them in the background of the main box. It doesn't need a new box underneath." LTX-2.5's and
// PAN2's boxes as the page draws them from the dev home's checkout, each mentioning a picture in its instructions, beside a box of the same
// tool written with the fields it had before (prompt, first_frame, duration_s), never rendered: at rest and in Focus, at 1440×900 and
// 1920×1080, paper and dark, in $OUT/web/video-mentions, with each tool's fields as Tools lists them and each box's drawn text.
// VM_BOXES: [{name, tool, fields}]; `<photo>` in a field names VM_PICTURE's box.
const { mkdirSync, readFileSync, writeFileSync } = require('fs');
const { join } = require('path');
const { test, expect, themed, focus, canvasIdle, canvasText, canvasVisible } = require('./fixtures');
const { fit, leave } = require('./hands');

test('video mentions: the boxes to look at', { tag: '@gpu' }, async ({ page, project, api }) => {
  const boxes = JSON.parse(process.env.VM_BOXES ?? '[]');
  test.skip(!boxes.length, 'release-gate/sessions/video-mentions.sh sets VM_BOXES');
  test.setTimeout(1_800_000);
  const shots = join(process.env.OUT, 'web', 'video-mentions');
  mkdirSync(shots, { recursive: true });
  const region = page.getByRole('region', { name: 'Focused asset' });
  if (process.env.VM_PICTURE) await project.import(readFileSync(process.env.VM_PICTURE), { name: 'photo', at: { x: 1150, y: 0 } });
  const listed = {};
  for (const [i, one] of boxes.entries()) {
    let tool = null;
    await expect.poll(async () => (tool = (await api.get(`/v1/tools?project=${encodeURIComponent(project.id)}`)).tools
      .find((held) => held.id === one.tool || held.id.endsWith(`/${one.tool}`)) ?? null)?.id ?? null,
    { message: `Tools lists ${one.tool}`, timeout: 900_000, intervals: [5_000] }).not.toBeNull();
    listed[one.name] = tool;
    await project.add({ name: one.name, tool: tool.id, kind: 'video', fields: one.fields, at: { x: (i % 2) * 560, y: Math.floor(i / 2) * 760 } });
  }
  writeFileSync(join(shots, 'tools.json'), JSON.stringify(listed, null, 2));
  await page.reload();
  await expect(page.locator('.stage')).toBeVisible();
  for (const one of boxes) await expect.poll(() => canvasVisible(page, one.name)).toBe(true);
  const drawn = {};
  for (const theme of ['paper', 'dark']) {
    await project.look(theme);
    await page.reload();
    await expect(page.locator('.stage')).toBeVisible();
    await themed(page, theme);
    for (const [width, height] of [[1440, 900], [1920, 1080]]) {
      if (await region.count()) await leave(page);
      await page.setViewportSize({ width, height });
      await canvasIdle(page);
      await fit(page);
      await page.screenshot({ path: join(shots, `rest-${width}x${height}-${theme}.png`) });
      for (const one of boxes) {
        drawn[one.name] ??= await canvasText(page, one.name);
        await focus(page, one.name);
        await page.screenshot({ path: join(shots, `focus-${one.name}-${width}x${height}-${theme}.png`) });
        await leave(page);
      }
    }
  }
  for (const one of boxes) drawn[`${one.name} (asset)`] = await project.asset(one.name);
  writeFileSync(join(shots, 'drawn.json'), JSON.stringify(drawn, null, 2));
  await project.look('paper');
});
