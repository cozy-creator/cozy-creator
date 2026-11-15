// nogpu (Paul, 11-15: "make sure the new pixijs canvas is robust"): a browser with no WebGL at all, as in a VM, over remote desktop, on a
// blocklisted driver or with WebGL turned off. The page is never blank: its bar, its tools and the canvas's region stand; in the region, in
// place and over nothing, it says why the boxes can't be drawn and what to do about it (a refusal names the next step); and the project's
// boxes can still be seen some way, its pictures on the screen. A picture of what such a person sees is kept with the cell's results.
const { test, expect, png, screenshotPixels } = require('./fixtures');

// No WebGL1 or WebGL2 context for any page: getContext('webgl'/'webgl2') answers null, so Pixi's isWebGLSupported() is false.
test.use({ launchOptions: { executablePath: process.env.CHROME, args: ['--disable-3d-apis'] } });

const PICTURES = { red: [217, 38, 38], green: [38, 217, 38], blue: [38, 38, 217] };

/** How many pixels of `colour` the canvas's region shows on the screen. */
async function shown(page, colour) {
  const region = await page.getByRole('region', { name: 'Project canvas' }).boundingBox();
  const read = screenshotPixels(await page.screenshot({ clip: region }));
  let count = 0;
  for (let y = 0; y < read.height; y += 2) for (let x = 0; x < read.width; x += 2) if (read(x, y).every((value, i) => Math.abs(value - colour[i]) <= 12)) count++;
  return count;
}

test('nogpu', async ({ page, project }, testInfo) => {
  expect(await page.evaluate(() => !document.createElement('canvas').getContext('webgl2') && !document.createElement('canvas').getContext('webgl')), 'this browser has no WebGL').toBe(true);
  for (const [i, [name, colour]] of Object.entries(PICTURES).entries()) await project.import(png(0, 320, 200, false, () => colour), { name, at: { x: i * 300, y: 0 } });
  await expect.poll(async () => (await project.assets()).length, { message: 'the boxes in the project' }).toBe(3);
  const region = page.getByRole('region', { name: 'Project canvas' });

  await test.step('not blank: the bar, the tools and the canvas region', async () => {
    await expect.soft(page.locator('header.top'), 'the bar').toBeVisible();
    await expect.soft(page.locator('.tools'), 'the tools').toBeVisible();
    await expect.soft(region, "the canvas's region").toBeVisible();
  });

  await test.step('in place, it says why and what to do', async () => {
    const notice = region.getByRole('alert');
    await expect(notice, 'a notice in the canvas region').toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('nogpu.png') });
    await testInfo.attach('nogpu', { path: testInfo.outputPath('nogpu.png'), contentType: 'image/png' });
    const words = await notice.innerText();
    expect.soft(words, 'why: the graphics').toMatch(/WebGL|graphics|GPU/i);
    expect.soft(words, 'what to do: a next step a person can take').toMatch(/hardware acceleration|turn on|enable|another browser|update|settings/i);
    await expect.soft(page.getByRole('dialog'), 'nothing over the page').toHaveCount(0);
    await expect.soft(page.getByRole('alertdialog')).toHaveCount(0);
  });

  await test.step("the project's pictures can still be seen", async () => {
    for (const [name, colour] of Object.entries(PICTURES)) {
      expect.soft(await shown(page, colour), `${name}'s picture on the screen`).toBeGreaterThan(400);
    }
    // And a way in by keyboard: the region's own help says arrows select and Enter opens.
    await region.focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Enter');
    await expect.soft(page.getByRole('region', { name: 'Focused asset' }), 'Enter opens a box in Focus').toBeVisible();
  });
});
