// references (from Codex's #3388; cl-1013 B1, acceptance rows 9 and 25): a box rendering on local whose words mention a picture draws that
// picture as its reference while it runs, the imported pixels in the crop, and again after a pan; repainting the crop never throws (the
// guards), and Cancel still works after the cropped references redraw.
const { createHash } = require('crypto');
const { test, expect, canvasCard, canvasIdle, focus, hoverBox, newCanvas, openPoint, partPoint, fixtureTool, png, screenshotPixels } = require('./fixtures');

test('references', async ({ page, project, api, cli }) => {
  await cli.json('machine', 'use', 'local');
  const bytes = png(90), source = screenshotPixels(bytes), sha = createHash('sha256').update(bytes).digest('hex');
  await project.import(bytes, { name: 'source', at: { x: 0, y: 0 } });
  await project.add({ name: 'preview', kind: 'image', tool: fixtureTool('tool.py/slow'), fields: { instructions: '<source>', seconds: 30 }, at: { x: 360, y: 0 } });
  await page.getByRole('button', { name: /more zoom options$/ }).click();
  await page.getByRole('menuitem', { name: /^Zoom to fit/ }).click();
  await canvasIdle(page);
  let run;
  const checkPicture = async (label) => {
    let picture;
    await expect.poll(async () => {
      picture = (await canvasCard(page, 'preview'))?.rendered?.references.find(reference => reference.sha === sha);
      return picture?.ready ?? false;
    }, { message: 'the running request draws its actual reference', timeout: 30000 }).toBe(true);
    const read = screenshotPixels(await page.screenshot({ path: test.info().outputPath(`${label}.png`), scale: 'css' }));
    const scale = Math.max(picture.width / source.width, picture.height / source.height);
    const crop = { width: picture.width / scale, height: picture.height / scale };
    const samples = [0.3, 0.7].map(u => {
      const v = 0.85;
      const expected = source((source.width - crop.width) / 2 + crop.width * u, (source.height - crop.height) / 2 + crop.height * v);
      const actual = read(picture.x + picture.width * u, picture.y + picture.height * v);
      return { actual, expected, error: Math.max(...actual.map((value, channel) => Math.abs(value - expected[channel]))) };
    });
    await test.info().attach(label, { body: Buffer.from(JSON.stringify({ picture, samples }, null, 2)), contentType: 'application/json' });
    for (const sample of samples) expect(sample.error, 'the cropped reference paints the imported source pixels').toBeLessThanOrEqual(32);
  };
  try {
    const editor = await focus(page, 'preview');
    await editor.getByRole('button', { name: 'Render', exact: true }).click();
    await expect.poll(async () => { [run] = (await project.asset('preview'))?.making ?? []; return run; }, { message: 'the local request starts', timeout: 120000 }).toBeTruthy();
    const started = await api.get(project.at(`/runs/${run}`));
    await test.info().attach('local-reference-run', { body: Buffer.from(JSON.stringify({ project: project.id, run, machine: started.machine, status: started.status }, null, 2)), contentType: 'application/json' });
    await page.keyboard.press('Escape');
    await expect(page.getByRole('region', { name: 'Focused asset' })).toHaveCount(0);
    // Nothing under the pointer: the tile as drawn at rest, not the card a hover mounts.
    const away = await openPoint(page);
    await page.mouse.move(away.x, away.y);
    await canvasIdle(page);
    await checkPicture('decoded-reference');
    const point = await openPoint(page);
    await page.mouse.move(point.x, point.y);
    await page.mouse.wheel(80, 0);
    await canvasIdle(page);
    await checkPicture('reference-after-pan');
    // Its Cancel: the old canvas's hover card's button, the new canvas's drawn one.
    if (await newCanvas(page)) {
      const at = await partPoint(page, 'preview', 'cancel');
      await page.mouse.click(at.x, at.y);
    } else {
      await hoverBox(page, 'preview');
      await page.getByRole('button', { name: "Cancel preview's render", exact: true }).click();
    }
    await expect.poll(async () => (await api.get(project.at(`/runs/${run}`))).status, { message: 'Cancel still works after cropped references redraw', timeout: 30000 }).toBe('canceled');
  } finally {
    if (run) {
      const state = await api.get(project.at(`/runs/${run}`));
      if (!['completed', 'failed', 'canceled'].includes(state.status)) {
        await cli.json('run', 'cancel', String(run));
        await expect.poll(async () => (await api.get(project.at(`/runs/${run}`))).status, { timeout: 30000 }).toMatch(/^(completed|failed|canceled)$/);
      }
    }
  }
});
